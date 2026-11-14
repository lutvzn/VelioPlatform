// The operator's page at velio.tv/ops (views/ops.tsx), built to
// public/static/ops.js. It calls the /ops API with the session cookie from
// signing in. Notices come from anyone on the web, so everything from the API
// goes into the page as text (textContent), never as HTML.
//
// Closing an account needs a reason and the account's email typed again; the
// button stays off until both are there, and the server checks the email
// again (POST /ops/accounts/:id/close, confirm).

interface NoticeSummary {
  id: string;
  received_at: number;
  kind: string;
  email: string;
  account_email: string;
  account_id: string | null;
  status: string;
}

interface Notice extends NoticeSummary {
  name: string;
  organization: string;
  work: string;
  sworn: number;
  signature: string;
  details: string;
  action: string;
  handled_at: number | null;
}

interface Account {
  id: string;
  email: string;
  created_at: number;
  confirmed_at: number | null;
  closed_at: number | null;
  closed_reason: string | null;
  purged_at: number | null;
  devices: number | null;
  /** What the operator gave it, or null for the default (the Rust server only). */
  tv_limit?: number | null;
  tv_limits?: { default: number; max: number; allowed: number };
  notices: { id: string; received_at: number; kind: string; status: string; action: string }[];
}

/** A row of GET /ops/accounts (the Rust server only). */
interface AccountRow {
  id: string;
  email: string;
  created_at: number;
  confirmed_at: number | null;
  closed_at: number | null;
  purged_at: number | null;
  tv_limit: number | null;
  devices: number;
  allowed: number;
  notice_count: number;
}

type Answer<T> = { ok: true; json: T } | { ok: false; status: number; problem: string };

const CLOSED_KEEP_MS = 30 * 86_400_000;

/** An element of the page (views/ops.tsx) that has to be there, of the kind expected. */
const element = <T extends Element>(kind: new () => T, selector: string, within: ParentNode = document) => {
  const found = within.querySelector(selector);
  if (!(found instanceof kind)) throw new Error(`The page has no ${selector}`);
  return found;
};
const find = (selector: string, within: ParentNode = document) => element(HTMLElement, selector, within);
const input = (selector: string) => {
  const found = find(selector);
  if (
    !(
      found instanceof HTMLInputElement ||
      found instanceof HTMLTextAreaElement ||
      found instanceof HTMLSelectElement
    )
  )
    throw new Error(`${selector} isn't a field`);
  return found;
};

type Content = Node | string | number | null | undefined | false;

/** An element with text and children; strings become text nodes, never HTML. */
const el = (
  tag: string,
  props: Record<string, string | ((event: Event) => void)> = {},
  ...children: (Content | Content[])[]
) => {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(props)) {
    if (typeof value === "function") node.addEventListener(key.replace(/^on/, ""), value);
    else if (key === "class") node.className = value;
    else node.setAttribute(key, value);
  }
  for (const child of children.flat()) {
    if (child === null || child === undefined || child === false) continue;
    node.append(typeof child === "string" || typeof child === "number" ? String(child) : child);
  }
  return node;
};

const when = (time: number | null | undefined) => (time ? new Date(time).toLocaleString() : "-");
const day = (time: number) => new Date(time).toLocaleDateString(undefined, { dateStyle: "long" });
const sameEmail = (a: string, b: string) =>
  a.trim() !== "" && a.trim().toLowerCase() === b.trim().toLowerCase();

const isObject = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null;

// What each answer has to look like (the /ops API in routes/ops.ts).
const listsNotices = (json: unknown): json is { notices: NoticeSummary[] } =>
  isObject(json) && Array.isArray(json.notices);
const hasNotice = (json: unknown): json is { notice: Notice; account: Account | null } =>
  isObject(json) && isObject(json.notice) && (json.account === null || isObject(json.account));
const listsAccounts = (json: unknown): json is { accounts: AccountRow[]; total: number } =>
  isObject(json) && Array.isArray(json.accounts) && typeof json.total === "number";
const hasAccount = (json: unknown): json is { account: Account } => isObject(json) && isObject(json.account);
const hasDeleteAt = (json: unknown): json is { deleteAt: number } =>
  isObject(json) && typeof json.deleteAt === "number";
const isOk = (json: unknown): json is { ok: true } => isObject(json) && json.ok === true;

const call = async <T>(
  path: string,
  expected: (json: unknown) => json is T,
  body?: Record<string, string | number | null>,
): Promise<Answer<T>> => {
  try {
    const init: RequestInit = { credentials: "same-origin" };
    if (body) {
      init.method = "POST";
      init.headers = { "Content-Type": "application/json" };
      init.body = JSON.stringify(body);
    }
    const answer = await fetch(`/ops${path}`, init);
    // The session ended (8 hours, or a new secret): back to signing in.
    if (answer.status === 401) {
      location.assign("/ops");
      return { ok: false, status: 401, problem: "Sign in again." };
    }
    const json: unknown = await answer.json().catch(() => null);
    if (answer.ok && expected(json)) return { ok: true, json };
    const problem =
      isObject(json) && typeof json.problem === "string"
        ? json.problem
        : `velio.tv answered ${answer.status}`;
    return { ok: false, status: answer.status, problem };
  } catch (error) {
    return { ok: false, status: 0, problem: `velio.tv didn't answer: ${String(error)}` };
  }
};

const say = (text: string, error = false) => {
  const status = find("#status");
  status.textContent = text;
  status.classList.toggle("error", error);
  status.hidden = false;
};

const describe = (items: [string, Content][]) =>
  el(
    "dl",
    {},
    items.flatMap(([term, value]) => [el("dt", {}, term), el("dd", {}, value === "" ? "-" : value)]),
  );

// --- What's shown, so it can be shown again after a change ---

let shownNotice: string | undefined;
let lookedUp: string | undefined;
const showAgain = async () => {
  if (shownNotice) await showNotice(shownNotice);
  if (lookedUp) await showAccount(lookedUp);
  await Promise.all([loadNotices(), loadAccounts()]);
};

// --- Tabs and signing out ---

for (const button of document.querySelectorAll<HTMLButtonElement>("[data-tab]"))
  button.addEventListener("click", () => {
    for (const other of document.querySelectorAll("[data-tab]"))
      other.setAttribute("aria-pressed", String(other === button));
    find("#notices").hidden = button.dataset.tab !== "notices";
    find("#accounts").hidden = button.dataset.tab !== "accounts";
    if (button.dataset.tab === "accounts") void loadAccounts();
  });

find("#signout").addEventListener("click", () => {
  void call("/signout", isOk, {}).then(() => location.assign("/ops"));
});

// --- Accounts ---

const accountState = (account: Account): Content => {
  if (account.purged_at)
    return el("span", { class: "closed" }, `Closed; data deleted ${when(account.purged_at)}`);
  if (account.closed_at)
    return el(
      "span",
      { class: "closed" },
      `Closed ${when(account.closed_at)}; data deleted on ${day(account.closed_at + CLOSED_KEEP_MS)}`,
    );
  return account.confirmed_at ? "Open" : "Open, email not confirmed";
};

const accountCard = (account: Account, noticeId?: string) => {
  const buttons: Node[] = [];
  if (!account.closed_at)
    buttons.push(
      el(
        "button",
        { type: "button", class: "danger", onclick: () => openClose(account, noticeId) },
        "Close this account…",
      ),
    );
  else if (!account.purged_at)
    buttons.push(el("button", { type: "button", onclick: () => openReopen(account) }, "Open it again…"));
  return el(
    "div",
    {},
    describe([
      ["Email", account.email],
      ["Account id", account.id],
      ["State", accountState(account)],
      ["Made", when(account.created_at)],
      ["Confirmed", when(account.confirmed_at)],
      ["TVs", tvCount(account)],
      ...(account.closed_reason ? [["Reason closed", account.closed_reason] as [string, Content]] : []),
    ]),
    el("h3", {}, `Notices naming it (${account.notices.length})`),
    account.notices.length === 0
      ? el("p", { class: "note" }, "None.")
      : el(
          "ul",
          {},
          account.notices.map(n =>
            el(
              "li",
              {},
              `${n.id}, ${n.kind}, ${when(n.received_at)}: ${n.status}${n.action ? ` (${n.action})` : ""}`,
            ),
          ),
        ),
    account.purged_at ? null : tvLimitForm(account),
    buttons.length ? el("div", { class: "actions" }, buttons) : null,
  );
};

// --- TV limits (POST /ops/accounts/:id/tv-limit) ---

const tvCount = (account: Account) => {
  const tvs = String(account.devices ?? 0);
  const limits = account.tv_limits;
  if (!limits) return tvs;
  return `${tvs} of ${limits.allowed} allowed${account.tv_limit == null ? " (the default)" : ""}`;
};

const setTvLimit = async (account: Account, limit: number | null) => {
  const answer = await call(`/accounts/${encodeURIComponent(account.id)}/tv-limit`, isOk, { limit });
  if (!answer.ok) {
    say(answer.problem, true);
    return;
  }
  const tvs = account.devices ?? 0;
  const allowed = limit ?? account.tv_limits?.default ?? 0;
  say(
    `${account.email} can now have ${allowed} TVs${limit === null ? " (the default)" : ""}.` +
      (tvs > allowed
        ? ` It has ${tvs}: they stay, and no more can be signed in until it's under ${allowed}.`
        : ""),
  );
  await showAgain();
};

const tvLimitForm = (account: Account) => {
  const limits = account.tv_limits;
  if (!limits) return null;
  const field = el("input", {
    type: "number",
    min: "1",
    max: String(limits.max),
    step: "1",
    required: "",
    value: String(limits.allowed),
  });
  const form = el(
    "form",
    { class: "bar tv-limit" },
    el("label", {}, "TV limit", field),
    el("button", { type: "submit" }, "Set"),
    account.tv_limit == null
      ? null
      : el(
          "button",
          { type: "button", onclick: () => void setTvLimit(account, null) },
          `Back to the default (${limits.default})`,
        ),
    el("span", { class: "note" }, `Default ${limits.default}, at most ${limits.max}.`),
  );
  form.addEventListener("submit", event => {
    event.preventDefault();
    if (!(field instanceof HTMLInputElement) || !field.reportValidity()) return;
    void setTvLimit(account, Number(field.value));
  });
  return form;
};

const showAccount = async (key: string) => {
  const card = find("#account");
  const answer = await call(`/accounts/${encodeURIComponent(key)}`, hasAccount);
  card.replaceChildren();
  card.hidden = false;
  if (!answer.ok) {
    card.append(el("p", { class: "problem" }, answer.problem));
    lookedUp = undefined;
    return;
  }
  lookedUp = answer.json.account.id;
  markAccountRow();
  card.append(el("h2", {}, answer.json.account.email), accountCard(answer.json.account));
};

find("#account-find").addEventListener("submit", event => {
  event.preventDefault();
  const key = input("#account-key").value.trim();
  if (key) void showAccount(key);
});

// --- The list of accounts (GET /ops/accounts; the Worker's page has no #account-rows) ---

const accountRows = document.querySelector<HTMLElement>("#account-rows");

const markAccountRow = () => {
  for (const row of accountRows?.querySelectorAll<HTMLElement>("tr") ?? [])
    row.setAttribute("aria-selected", String(row.dataset.accountId === lookedUp));
};

const openAccountRow = async (id: string) => {
  await showAccount(id);
  find("#account").scrollIntoView({ behavior: "smooth", block: "nearest" });
};

/** Only the latest list is shown, when answers come back out of order. */
let listing = 0;
const loadAccounts = async () => {
  if (!accountRows) return;
  const asked = ++listing;
  const query = new URLSearchParams({
    state: input("#account-state").value,
    q: input("#account-key").value.trim(),
  });
  const answer = await call(`/accounts?${query.toString()}`, listsAccounts);
  if (asked !== listing) return;
  accountRows.replaceChildren();
  const count = find("#account-count");
  if (!answer.ok) {
    count.textContent = answer.problem;
    return;
  }
  const { accounts, total } = answer.json;
  count.textContent =
    total === 0
      ? "No accounts."
      : accounts.length < total
        ? `The newest ${accounts.length} of ${total}. Type to narrow it.`
        : `${total} ${total === 1 ? "account" : "accounts"}.`;
  for (const account of accounts)
    accountRows.append(
      el(
        "tr",
        {
          tabindex: "0",
          "data-account-id": account.id,
          "aria-selected": String(lookedUp === account.id),
          onclick: () => void openAccountRow(account.id),
          onkeydown: event => {
            if (event instanceof KeyboardEvent && event.key === "Enter") void openAccountRow(account.id);
          },
        },
        el("td", {}, account.email),
        el("td", {}, when(account.created_at)),
        el(
          "td",
          {},
          account.closed_at
            ? account.purged_at
              ? "Closed, deleted"
              : "Closed"
            : account.confirmed_at
              ? "Open"
              : "Not confirmed",
        ),
        el("td", {}, `${account.devices} of ${account.allowed}${account.tv_limit == null ? "" : " (given)"}`),
        el("td", {}, account.notice_count || "-"),
      ),
    );
};

if (accountRows) {
  let typing: ReturnType<typeof setTimeout> | undefined;
  find("#account-key").addEventListener("input", () => {
    clearTimeout(typing);
    typing = setTimeout(() => void loadAccounts(), 250);
  });
  find("#account-state").addEventListener("change", () => void loadAccounts());
}

// --- Notices ---

const loadNotices = async () => {
  const answer = await call(
    `/notices?status=${encodeURIComponent(input("#notice-status").value)}`,
    listsNotices,
  );
  const rows = find("#notice-rows");
  rows.replaceChildren();
  if (!answer.ok) {
    say(answer.problem, true);
    return;
  }
  find("#notice-empty").hidden = answer.json.notices.length > 0;
  for (const n of answer.json.notices)
    rows.append(
      el(
        "tr",
        {
          tabindex: "0",
          "data-notice-id": n.id,
          "aria-selected": String(shownNotice === n.id),
          onclick: () => void showNotice(n.id),
          onkeydown: event => {
            if (event instanceof KeyboardEvent && event.key === "Enter") void showNotice(n.id);
          },
        },
        el("td", {}, n.id),
        el("td", {}, when(n.received_at)),
        el("td", {}, n.kind),
        el("td", {}, n.email),
        el("td", {}, n.account_email ? `${n.account_email}${n.account_id ? "" : " (no such account)"}` : "-"),
        el("td", {}, n.status),
      ),
    );
};

const showNotice = async (id: string) => {
  const card = find("#notice");
  const answer = await call(`/notices/${encodeURIComponent(id)}`, hasNotice);
  card.replaceChildren();
  card.hidden = false;
  if (!answer.ok) {
    card.append(el("p", { class: "problem" }, answer.problem));
    return;
  }
  shownNotice = id;
  for (const row of document.querySelectorAll<HTMLElement>("#notice-rows tr"))
    row.setAttribute("aria-selected", String(row.dataset.noticeId === id));
  const { notice, account } = answer.json;
  const handle = (status: "rejected" | "actioned") =>
    el("button", { type: "button", onclick: () => openHandle(notice, status) }, `Mark ${status}…`);
  card.append(
    el("h2", {}, `Notice ${notice.id}`),
    describe([
      ["Status", notice.status],
      ...(notice.action ? [["What was done", notice.action] as [string, Content]] : []),
      ...(notice.handled_at ? [["Handled", when(notice.handled_at)] as [string, Content]] : []),
      ["Received", when(notice.received_at)],
      ["Kind", notice.kind],
      ["From", `${notice.name} <${notice.email}>`],
      ["Organization", notice.organization],
      ["Account named", notice.account_email],
      ...(notice.kind === "copyright"
        ? ([
            ["Work", notice.work],
            ["Sworn statements", notice.sworn ? "Yes" : "No"],
            ["Signature", notice.signature],
          ] as [string, Content][])
        : []),
    ]),
    el("h3", {}, "Details"),
    el("div", { class: "details" }, notice.details),
    ...(notice.status === "open"
      ? [el("div", { class: "actions" }, handle("rejected"), handle("actioned"))]
      : []),
    el("h3", {}, "The account it names"),
    account
      ? accountCard(account, notice.id)
      : el(
          "p",
          { class: "note" },
          notice.account_email ? "No account has that email." : "It names no account.",
        ),
  );
};

find("#notice-status").addEventListener("change", () => void loadNotices());
find("#notice-refresh").addEventListener("click", () => void loadNotices());

// --- Dialogs ---

for (const dialog of document.querySelectorAll("dialog"))
  find("[data-cancel]", dialog).addEventListener("click", () => dialog.close());

const problemIn = (dialog: HTMLDialogElement, text: string) => {
  find("[data-problem]", dialog).textContent = text;
};

let closing: Account | undefined;
const closeDialog = element(HTMLDialogElement, "#close-dialog");
const closeSubmit = element(HTMLButtonElement, "#close-submit");
const closeReady = () =>
  closing !== undefined &&
  sameEmail(input("#close-confirm").value, closing.email) &&
  input("#close-reason").value.trim() !== "";
const updateClose = () => {
  closeSubmit.disabled = !closeReady();
};

const openClose = (account: Account, noticeId?: string) => {
  closing = account;
  for (const span of closeDialog.querySelectorAll("[data-email]")) span.textContent = account.email;
  find("[data-delete-on]", closeDialog).textContent = day(Date.now() + CLOSED_KEEP_MS);
  find("[data-account-facts]", closeDialog).textContent =
    `Made ${when(account.created_at)}. ${account.devices ?? 0} TV(s). Named in ${account.notices.length} notice(s).`;
  input("#close-reason").value = "";
  input("#close-notice").value = noticeId ?? "";
  input("#close-confirm").value = "";
  problemIn(closeDialog, "");
  updateClose();
  closeDialog.showModal();
};
find("#close-reason").addEventListener("input", updateClose);
find("#close-confirm").addEventListener("input", updateClose);
find("#close-form").addEventListener("submit", event => {
  event.preventDefault();
  const account = closing;
  if (!account || !closeReady()) return;
  closeSubmit.disabled = true;
  void (async () => {
    const answer = await call(`/accounts/${encodeURIComponent(account.id)}/close`, hasDeleteAt, {
      reason: input("#close-reason").value,
      notice: input("#close-notice").value.trim(),
      confirm: input("#close-confirm").value,
    });
    if (!answer.ok) {
      problemIn(closeDialog, answer.problem);
      updateClose();
      return;
    }
    closeDialog.close();
    closing = undefined;
    say(
      `Closed ${account.email}. The owner was emailed. Its data will be deleted on ${day(answer.json.deleteAt)}.`,
    );
    await showAgain();
  })();
});

let reopening: Account | undefined;
const reopenDialog = element(HTMLDialogElement, "#reopen-dialog");
const openReopen = (account: Account) => {
  reopening = account;
  for (const span of reopenDialog.querySelectorAll("[data-email]")) span.textContent = account.email;
  problemIn(reopenDialog, "");
  reopenDialog.showModal();
};
find("#reopen-form").addEventListener("submit", event => {
  event.preventDefault();
  const account = reopening;
  if (!account) return;
  void (async () => {
    const answer = await call(`/accounts/${encodeURIComponent(account.id)}/reopen`, isOk, {});
    if (!answer.ok) {
      problemIn(reopenDialog, answer.problem);
      return;
    }
    reopenDialog.close();
    say(`Opened ${account.email} again. The owner was emailed.`);
    await showAgain();
  })();
});

let handling: { id: string; status: "rejected" | "actioned" } | undefined;
const handleDialog = element(HTMLDialogElement, "#handle-dialog");
const openHandle = (notice: Notice, status: "rejected" | "actioned") => {
  handling = { id: notice.id, status };
  find("[data-notice]", handleDialog).textContent = notice.id;
  find("[data-handle-status]", handleDialog).textContent = status;
  input("#handle-action").value = "";
  problemIn(handleDialog, "");
  handleDialog.showModal();
};
find("#handle-form").addEventListener("submit", event => {
  event.preventDefault();
  const target = handling;
  const action = input("#handle-action").value.trim();
  if (!target) return;
  if (!action) {
    problemIn(handleDialog, "Say what was done.");
    return;
  }
  void (async () => {
    const answer = await call(`/notices/${encodeURIComponent(target.id)}`, isOk, {
      status: target.status,
      action,
    });
    if (!answer.ok) {
      problemIn(handleDialog, answer.problem);
      return;
    }
    handleDialog.close();
    say(`Notice ${target.id} marked ${target.status}.`);
    await showAgain();
  })();
});

void loadNotices();
