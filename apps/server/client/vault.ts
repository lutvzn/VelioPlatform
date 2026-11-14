// vault.js: everything secret in the browser (PLAN.md, "HTMX and end-to-end
// encryption"). Forms marked data-velio="…" have hx-confirm, so HTMX hands
// us the request before sending it:
//
//   signup, reset          a new password becomes keys and a new owner bundle
//   signin, unlock         the password becomes the auth key; the answer
//                          brings the owner bundle, opened with the unlock key
//   password               the current password's auth key, and the same
//                          owner keys sealed under a new password
//   recovery-on, -off      a new recovery key, shown once, and the owner
//                          keys sealed under it; or turning it off
//   recover                from the reset link: the recovery key opens its
//                          copy of the owner keys, sealed under a new password
//   delete-account         the current password's auth key
//   profile-…, list-…,     the edit is made on the opened account and sent
//   device-…
//                          as one sealed, signed change
//   pair-find, pair        signing in a TV: the pairing id and a proof of its
//                          secret, then the change that adds the TV and the
//                          owner's public keys sealed for it
//
// Inputs that hold anything secret have no name, so HTMX never sends them.
// After every swap, pages' ciphertext ([data-vault-state]) is opened and
// the names and fields filled in.
//
// The owner's keys stay in this tab's memory, which boosted links keep
// between pages. "Remember this browser" also keeps them in IndexedDB,
// encrypted with a non-extractable AES key, until signing out.

import {
  buildChange,
  ChangeProblem,
  emailProblem,
  exportFileName,
  exportLists,
  fromBase64,
  isRecord,
  isSealed,
  LIST_PROBLEM_TEXT,
  listProblem,
  MAX_NAME_LENGTH,
  newOwnerKeys,
  openDeviceInfo,
  openOwnerBundle,
  openState,
  ownerFromSecrets,
  ownerPublicKeys,
  ownerSecrets,
  pairingProof,
  PROFILE_COLORS,
  PROFILE_LETTER_COLOR,
  PROFILE_PROBLEM_TEXT,
  profileColor,
  profileInitial,
  profileProblem,
  readPairingCode,
  readStoredState,
  sealOwnerBundle,
  sealPairingPayload,
  splitGuideUrls,
  toBase64,
  withDevice,
  withDeviceProfiles,
  withList,
  withoutDevice,
  withoutList,
  withoutProfile,
  withProfile,
  type DeviceInfo,
  type ListDraft,
  type OpenedList,
  type OpenedState,
  type OwnerKeys,
  type ProfileDraft,
  type ReadCode,
} from "@velio/sync-core";
import {
  DEFAULT_KDF,
  deriveLoginKeys,
  deriveRecoveryKeys,
  formatRecoveryKey,
  newRecoveryKey,
  newSalt,
  passwordProblem,
  readRecoveryKey,
  type LoginKeys,
} from "@velio/sync-core/login";

declare global {
  interface Window {
    turnstile?: { reset: () => void; remove: (container: Element) => void };
    velioVault?: { unlocked: () => boolean };
  }
}

type Body = URLSearchParams | FormData;

/** The part of HTMX 4's htmx:confirm detail used here. */
interface ConfirmDetail {
  body: Body | null;
  form: HTMLFormElement;
  issueRequest: () => void;
  dropRequest: () => void;
}

const readConfirm = (event: Event): ConfirmDetail | null => {
  if (!(event instanceof CustomEvent)) return null;
  const detail: unknown = event.detail;
  if (!isRecord(detail) || !isRecord(detail.ctx) || !isRecord(detail.ctx.request)) return null;
  const { issueRequest, dropRequest } = detail;
  const form = detail.ctx.sourceElement;
  const body = detail.ctx.request.body;
  if (typeof issueRequest !== "function" || typeof dropRequest !== "function") return null;
  if (!(form instanceof HTMLFormElement)) return null;
  return {
    body: body instanceof URLSearchParams || body instanceof FormData ? body : null,
    form,
    issueRequest: () => issueRequest(),
    dropRequest: () => dropRequest(),
  };
};

// --- What this tab holds ---

let owner: OwnerKeys | null = null;
/** The account the keys are for. */
let ownerAccount: string | null = null;
/** Whether IndexedDB holds them too. */
let remembered = false;
/** The page's account, opened: what changes are built on. */
let opened: OpenedState | null = null;
/** The unlock key from the last sign-in or unlock, until the answer brings the owner bundle. */
let pendingUnlockKey: Uint8Array | null = null;
let pendingRemember = false;
/** A recovery key just made, until the answer shows where to put it. */
let pendingRecoveryKey: string | null = null;

/** The signed-in account, from the nav; none when signed out. */
const pageAccount = () => document.querySelector<HTMLElement>("#nav")?.dataset.accountId ?? null;

// --- Remember this browser ---

const DB_NAME = "velio";
const STORE = "keys";
const ENTRY = "owner";

interface RememberedEntry {
  accountId: string;
  key: CryptoKey;
  iv: Uint8Array<ArrayBuffer>;
  data: ArrayBuffer;
}

const isRemembered = (value: unknown): value is RememberedEntry =>
  isRecord(value) &&
  typeof value.accountId === "string" &&
  value.key instanceof CryptoKey &&
  value.iv instanceof Uint8Array &&
  value.data instanceof ArrayBuffer;

const withStore = async <T>(mode: IDBTransactionMode, run: (store: IDBObjectStore) => IDBRequest<T>) => {
  const db = await new Promise<IDBDatabase>((resolve, reject) => {
    const request = indexedDB.open(DB_NAME, 1);
    request.addEventListener("upgradeneeded", () => request.result.createObjectStore(STORE));
    request.addEventListener("success", () => resolve(request.result));
    request.addEventListener("error", () => reject(request.error ?? new Error("IndexedDB didn't open")));
  });
  try {
    return await new Promise<T>((resolve, reject) => {
      const transaction = db.transaction(STORE, mode);
      const request = run(transaction.objectStore(STORE));
      const failed = () => reject(transaction.error ?? new Error("IndexedDB failed"));
      transaction.addEventListener("complete", () => resolve(request.result));
      transaction.addEventListener("error", failed);
      transaction.addEventListener("abort", failed);
    });
  } finally {
    db.close();
  }
};

const remember = async (accountId: string, keys: OwnerKeys) => {
  const key = await crypto.subtle.generateKey({ name: "AES-GCM", length: 256 }, false, [
    "encrypt",
    "decrypt",
  ]);
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const plain = new TextEncoder().encode(JSON.stringify(ownerSecrets(keys)));
  const data = await crypto.subtle.encrypt({ name: "AES-GCM", iv }, key, plain);
  const entry: RememberedEntry = { accountId, key, iv, data };
  await withStore("readwrite", store => store.put(entry, ENTRY));
  remembered = true;
};

const recall = async (accountId: string): Promise<OwnerKeys | null> => {
  const entry: unknown = await withStore("readonly", store => store.get(ENTRY));
  if (!isRemembered(entry) || entry.accountId !== accountId) return null;
  const plain = await crypto.subtle.decrypt({ name: "AES-GCM", iv: entry.iv }, entry.key, entry.data);
  return ownerFromSecrets(JSON.parse(new TextDecoder().decode(plain)));
};

const forget = async () => {
  remembered = false;
  try {
    await withStore("readwrite", store => store.delete(ENTRY));
  } catch (error) {
    console.error(error);
  }
};

const lock = () => {
  owner = null;
  ownerAccount = null;
  opened = null;
  void forget();
};

// --- Forms ---

const valueOf = (form: HTMLFormElement, selector: string) =>
  form.querySelector<HTMLInputElement | HTMLSelectElement | HTMLTextAreaElement>(selector)?.value ?? "";

const passwordOf = (form: HTMLFormElement, selector: string) => valueOf(form, selector);

const showProblem = (form: HTMLFormElement, message: string) => {
  const box = form.querySelector(".problem[role=alert]") ?? document.querySelector("#form-problem");
  if (box) box.textContent = message;
};

const setBusy = (form: HTMLFormElement, busy: boolean) => {
  form.toggleAttribute("aria-busy", busy);
  const button = form.querySelector<HTMLButtonElement>("button[type=submit]");
  if (button) button.disabled = busy;
};

/** A problem to show, null when the owner said no, or undefined to send the form. */
type Outcome = string | null | undefined;

const LOCKED = "Your keys are locked. Reload the page and unlock.";
const NO_ANSWER = "Velio didn't answer. Try again in a minute.";

/** A new password, checked twice, into a new salt and keys, written in the body; or a problem. */
const newLogin = async (form: HTMLFormElement, body: Body): Promise<LoginKeys | string> => {
  const password = passwordOf(form, "[data-password]");
  const problem = passwordProblem(password);
  if (problem) return problem;
  if (password !== passwordOf(form, "[data-password-again]")) return "The two passwords aren't the same.";
  const salt = newSalt();
  const keys = await deriveLoginKeys(password, salt, DEFAULT_KDF);
  body.set("salt", toBase64(salt));
  body.set("kdf_memory_kib", String(DEFAULT_KDF.memoryKiB));
  body.set("kdf_iterations", String(DEFAULT_KDF.iterations));
  body.set("kdf_parallelism", String(DEFAULT_KDF.parallelism));
  body.set("auth_key", toBase64(keys.authKey));
  return keys;
};

/** The owner's keys, sealed under an unlock key, with their public halves, in the body. */
const setBundle = (body: Body, unlockKey: Uint8Array, keys: OwnerKeys) => {
  const publicKeys = ownerPublicKeys(keys);
  body.set("bundle", JSON.stringify(sealOwnerBundle(unlockKey, keys)));
  body.set("sign_public_key", publicKeys.signPublicKey);
  body.set("box_public_key", publicKeys.boxPublicKey);
};

/** A new password, checked twice, turned into everything a new or reset account needs. */
const newCredentials = async (form: HTMLFormElement, body: Body): Promise<Outcome> => {
  const keys = await newLogin(form, body);
  if (typeof keys === "string") return keys;
  setBundle(body, keys.unlockKey, newOwnerKeys());
  pendingUnlockKey = keys.unlockKey;
  // Whatever this browser remembered was for the old keys.
  await forget();
  return undefined;
};

/** The account's salt and settings, then the password into its login keys; or a problem. */
const loginKeysFor = async (email: string, password: string): Promise<LoginKeys | string> => {
  const problem = emailProblem(email);
  if (problem) return problem;
  const response = await fetch(`/prelogin?email=${encodeURIComponent(email)}`, {
    headers: { Accept: "application/json" },
  });
  const answer: unknown = await response.json().catch(() => null);
  if (!isRecord(answer)) return NO_ANSWER;
  const { salt, memoryKiB, iterations, parallelism } = answer;
  if (!response.ok || typeof salt !== "string")
    return typeof answer.problem === "string" ? answer.problem : NO_ANSWER;
  if (typeof memoryKiB !== "number" || typeof iterations !== "number" || typeof parallelism !== "number")
    return "Velio's answer wasn't readable. Try again in a minute.";
  // deriveLoginKeys refuses settings weaker than the default, so a server can't make guessing cheaper.
  return deriveLoginKeys(password, fromBase64(salt), { memoryKiB, iterations, parallelism });
};

const signinCredentials = async (form: HTMLFormElement, body: Body): Promise<Outcome> => {
  const typed = body.get("email");
  const keys = await loginKeysFor(
    typeof typed === "string" ? typed : "",
    passwordOf(form, "[data-password]"),
  );
  if (typeof keys === "string") return keys;
  body.set("auth_key", toBase64(keys.authKey));
  pendingUnlockKey = keys.unlockKey;
  pendingRemember = form.querySelector<HTMLInputElement>("[data-remember]")?.checked === true;
  return undefined;
};

/** For settings: the signed-in account's current password, as its auth key. */
const currentPassword = async (form: HTMLFormElement, body: Body): Promise<Outcome> => {
  const keys = await loginKeysFor(form.dataset.email ?? "", passwordOf(form, "[data-password-current]"));
  if (typeof keys === "string") return keys;
  body.set("current_auth_key", toBase64(keys.authKey));
  return undefined;
};

/** The same owner keys under a new password. This tab, and a remembering browser, keep them as they are. */
const changePassword = async (form: HTMLFormElement, body: Body): Promise<Outcome> => {
  if (!owner) return LOCKED;
  const keys = owner;
  const problem = passwordProblem(passwordOf(form, "[data-password]"));
  if (problem) return problem;
  const current = await currentPassword(form, body);
  if (current !== undefined) return current;
  const login = await newLogin(form, body);
  if (typeof login === "string") return login;
  setBundle(body, login.unlockKey, keys);
  return undefined;
};

const recoveryOn = async (form: HTMLFormElement, body: Body): Promise<Outcome> => {
  if (!owner) return LOCKED;
  const keys = owner;
  const current = await currentPassword(form, body);
  if (current !== undefined) return current;
  const recoveryKey = newRecoveryKey();
  const recovery = await deriveRecoveryKeys(recoveryKey);
  body.set("recovery_auth_key", toBase64(recovery.authKey));
  body.set("recovery_bundle", JSON.stringify(sealOwnerBundle(recovery.unlockKey, keys)));
  pendingRecoveryKey = recoveryKey;
  return undefined;
};

const deleteAccount = async (form: HTMLFormElement, body: Body): Promise<Outcome> => {
  if (!window.confirm("Delete your account, with its lists and profiles? Every TV is signed out."))
    return null;
  return currentPassword(form, body);
};

/** From the reset link: the recovery key opens its copy of the owner keys, which a new password seals. */
const recover = async (form: HTMLFormElement, body: Body): Promise<Outcome> => {
  const recoveryKey = readRecoveryKey(valueOf(form, "[data-recovery-key]"));
  if (!recoveryKey) return "That isn't a recovery key. It has 32 letters and digits, such as K7QM-2XHF-….";
  const problem = passwordProblem(passwordOf(form, "[data-password]"));
  if (problem) return problem;
  const recovery = await deriveRecoveryKeys(recoveryKey);
  const recoveryAuthKey = toBase64(recovery.authKey);
  const response = await fetch(`/reset/${encodeURIComponent(form.dataset.token ?? "")}/recovery-bundle`, {
    method: "POST",
    headers: { Accept: "application/json" },
    body: new URLSearchParams({ recovery_auth_key: recoveryAuthKey }),
  });
  const answer: unknown = await response.json().catch(() => null);
  if (!isRecord(answer)) return NO_ANSWER;
  if (!response.ok) return typeof answer.problem === "string" ? answer.problem : NO_ANSWER;
  const { bundle, signPublicKey, boxPublicKey } = answer;
  if (!isSealed(bundle) || typeof signPublicKey !== "string" || typeof boxPublicKey !== "string")
    return "Velio's answer wasn't readable. Try again in a minute.";
  let keys: OwnerKeys;
  try {
    keys = openOwnerBundle(recovery.unlockKey, bundle, { signPublicKey, boxPublicKey });
  } catch {
    return "The recovery key didn't open your keys. Check it, and try again.";
  }
  const login = await newLogin(form, body);
  if (typeof login === "string") return login;
  setBundle(body, login.unlockKey, keys);
  body.set("recovery_auth_key", recoveryAuthKey);
  pendingUnlockKey = login.unlockKey;
  await forget();
  return undefined;
};

const credentials: Record<string, (form: HTMLFormElement, body: Body) => Promise<Outcome>> = {
  signup: newCredentials,
  reset: newCredentials,
  signin: signinCredentials,
  unlock: signinCredentials,
  password: changePassword,
  "recovery-on": recoveryOn,
  "recovery-off": currentPassword,
  recover,
  "delete-account": deleteAccount,
};

const sendCredentials = async (
  run: (form: HTMLFormElement, body: Body) => Promise<Outcome>,
  detail: ConfirmDetail,
  body: Body,
) => {
  const { form } = detail;
  showProblem(form, "");
  setBusy(form, true);
  pendingUnlockKey = null;
  pendingRemember = false;
  pendingRecoveryKey = null;
  try {
    const outcome = await run(form, body);
    if (outcome === undefined) detail.issueRequest();
    else {
      if (outcome) showProblem(form, outcome);
      detail.dropRequest();
    }
  } catch (error) {
    console.error(error);
    showProblem(form, "This browser couldn't make the keys. Try another browser, or update this one.");
    detail.dropRequest();
  } finally {
    setBusy(form, false);
  }
};

// --- Changes ---

const nameProblem = (name: string) => {
  if (!name) return "Enter a name.";
  if (name.length > MAX_NAME_LENGTH) return "That name is too long.";
  return undefined;
};

const readDraft = (form: HTMLFormElement): ListDraft => {
  const field = (name: string) => valueOf(form, `[data-field="${name}"]`).trim();
  const chosen = field("kind");
  const kind = chosen === "xtream" || chosen === "stremio" ? chosen : "m3u";
  const draft: ListDraft = { name: field("name"), kind, refreshHours: Number(field("refreshHours")) };
  if (kind === "m3u") draft.url = field("url");
  else if (kind === "stremio") {
    draft.url = field("addonUrl");
    // An add-on has no guide, whatever the hidden field holds.
    return form.querySelector<HTMLInputElement>('[data-field="disabled"]')?.checked
      ? { ...draft, disabled: true }
      : draft;
  } else {
    draft.server = field("server");
    draft.username = field("username");
    draft.password = field("password");
  }
  const guideUrls = splitGuideUrls(field("guideUrls"));
  if (guideUrls.length > 0) draft.guideUrls = guideUrls;
  if (form.querySelector<HTMLInputElement>('[data-field="disabled"]')?.checked) draft.disabled = true;
  return draft;
};

const readProfileDraft = (form: HTMLFormElement): ProfileDraft => {
  const draft: ProfileDraft = { name: valueOf(form, '[data-field="name"]').trim() };
  const color = form.querySelector<HTMLInputElement>("[data-color-choice]:checked");
  if (color) draft.color = Number(color.value);
  const pin = valueOf(form, '[data-field="pin"]').trim();
  draft.pin = pin === "" ? null : pin;
  return draft;
};

const chosenProfiles = (form: HTMLFormElement) =>
  [...form.querySelectorAll<HTMLInputElement>("[data-profile-choice]")]
    .filter(input => input.checked)
    .map(input => input.dataset.profileChoice ?? "");

/** An edit: the account after it, a problem to show, or null when the owner said no. */
type Edit = (form: HTMLFormElement, state: OpenedState) => OpenedState | string | null;

const edits: Record<string, Edit> = {
  "profile-add": (form, state) => {
    const name = valueOf(form, '[data-field="name"]').trim();
    return nameProblem(name) ?? withProfile(state, { name }).state;
  },
  "profile-edit": (form, state) => {
    const draft = readProfileDraft(form);
    const problem = profileProblem(draft);
    return problem ? PROFILE_PROBLEM_TEXT[problem] : withProfile(state, draft, form.dataset.profileId).state;
  },
  "profile-delete": (form, state) =>
    window.confirm("Delete this profile? Its lists stay in your account.")
      ? withoutProfile(state, form.dataset.profileId ?? "")
      : null,
  "list-save": (form, state) => {
    const draft = readDraft(form);
    const problem = listProblem(draft);
    if (problem) return LIST_PROBLEM_TEXT[problem];
    return withList(state, draft, chosenProfiles(form), form.dataset.listId).state;
  },
  "device-profiles": (form, state) =>
    withDeviceProfiles(state, form.dataset.deviceId ?? "", chosenProfiles(form)),
  "device-remove": (form, state) =>
    window.confirm("Remove this TV? It signs out the next time it syncs.")
      ? withoutDevice(state, form.dataset.deviceId ?? "")
      : null,
  "list-delete": (form, state) =>
    window.confirm("Delete this list? TVs lose it the next time they sync.")
      ? withoutList(state, form.dataset.listId ?? "")
      : null,
};

const sendChange = (edit: Edit, detail: ConfirmDetail, body: Body) => {
  const { form } = detail;
  showProblem(form, "");
  if (!owner || !opened) {
    showProblem(form, "Your keys are locked. Reload the page and unlock.");
    detail.dropRequest();
    return;
  }
  try {
    const after = edit(form, opened);
    if (after === null || typeof after === "string") {
      if (after) showProblem(form, after);
      detail.dropRequest();
      return;
    }
    // Only the change and where to go next leave the page, whatever the form named.
    const others = Array.from(body.keys()).filter(key => key !== "next");
    for (const key of others) body.delete(key);
    body.set("change", JSON.stringify(buildChange(owner, opened, after)));
    detail.issueRequest();
  } catch (error) {
    if (!(error instanceof ChangeProblem)) console.error(error);
    showProblem(
      form,
      error instanceof ChangeProblem ? error.message : "This change couldn't be made. Reload the page.",
    );
    detail.dropRequest();
  }
};

// --- Signing in a TV ---

/** The code from the TV's QR code or typed in, until the TV is signed in. */
let pairCode: ReadCode | null = null;
/** The TV's key and name, opened with that code. */
let pairDevice: DeviceInfo | null = null;

/** The QR code's fragment, kept for this tab while signing in first. */
const PAIR_STORAGE = "velio-pair";

const storage = {
  get: () => {
    try {
      return sessionStorage.getItem(PAIR_STORAGE);
    } catch {
      return null;
    }
  },
  set: (value: string | null) => {
    try {
      if (value === null) sessionStorage.removeItem(PAIR_STORAGE);
      else sessionStorage.setItem(PAIR_STORAGE, value);
    } catch {
      // Without storage, scanning the code again after signing in works too.
    }
  },
};

/** /pair#K7QM.<secret>: keep the fragment, and take it out of the address bar and history. */
const capturePairCode = () => {
  if (location.pathname !== "/pair" || location.hash.length < 2) return;
  const fragment = decodeURIComponent(location.hash.slice(1));
  history.replaceState(null, "", "/pair");
  if (readPairingCode(fragment)) storage.set(fragment);
};

const setProof = (body: Body, code: ReadCode) => {
  body.set("kind", code.kind);
  body.set("proof", pairingProof(code.kind, code.secret));
};

const findPairing = (detail: ConfirmDetail, body: Body) => {
  const { form } = detail;
  showProblem(form, "");
  const code = readPairingCode(valueOf(form, '[data-field="code"]'));
  if (!code) {
    showProblem(form, "That isn't a code from VelioTV. It looks like K7QM-2XHF-9WRT.");
    detail.dropRequest();
    return;
  }
  pairCode = code;
  pairDevice = null;
  body.set("id", code.id);
  setProof(body, code);
  detail.issueRequest();
};

const sendPairing = (detail: ConfirmDetail, body: Body) => {
  const { form } = detail;
  showProblem(form, "");
  const deviceId = form.dataset.deviceId ?? "";
  if (!owner || !opened || !ownerAccount) {
    showProblem(form, "Your keys are locked. Reload the page and unlock.");
    detail.dropRequest();
    return;
  }
  if (!pairCode || !pairDevice) {
    showProblem(form, "Find the TV again with its code.");
    detail.dropRequest();
    return;
  }
  try {
    // The account's TV limit, from the server (without it, the default).
    const tvLimit = Number(form.dataset.tvLimit) || undefined;
    const after = withDevice(
      opened,
      deviceId,
      fromBase64(pairDevice.publicKey),
      chosenProfiles(form),
      pairDevice.name,
      tvLimit,
    );
    body.set("change", JSON.stringify(buildChange(owner, opened, after)));
    const payload = sealPairingPayload(pairCode, {
      format: 1,
      account: ownerAccount,
      device: deviceId,
      devicePublicKey: pairDevice.publicKey,
      ...ownerPublicKeys(owner),
    });
    body.set("payload", JSON.stringify(payload));
    setProof(body, pairCode);
    detail.issueRequest();
  } catch (error) {
    if (!(error instanceof ChangeProblem)) console.error(error);
    showProblem(
      form,
      error instanceof ChangeProblem ? error.message : "The TV couldn't be signed in. Reload the page.",
    );
    detail.dropRequest();
  }
};

/** Opens the found TV's details with the code, and fills in the code from a QR code once the keys are open. */
const showPairing = () => {
  // Signed in: the code did its job. Once per TV signed in, as finding the
  // next TV swaps only #pair-step, and this status stays on the page.
  const done = document.querySelector<HTMLElement>("#main .done[role=status]:not([data-seen])");
  if (done) {
    done.dataset.seen = "yes";
    storage.set(null);
    pairCode = null;
    pairDevice = null;
  }

  const section = document.querySelector<HTMLElement>("[data-sealed-device]");
  if (section && !section.dataset.opened) {
    section.dataset.opened = "yes";
    pairDevice = null;
    const form = section.querySelector<HTMLFormElement>('form[data-velio="pair"]');
    try {
      const sealed: unknown = JSON.parse(section.dataset.sealedDevice ?? "");
      if (!pairCode || section.dataset.kind !== pairCode.kind) throw new Error("No code for this TV");
      if (!isRecord(sealed) || !isSealed(sealed.qr) || !isSealed(sealed.code))
        throw new Error("The TV's details aren't readable");
      pairDevice = openDeviceInfo(pairCode, { qr: sealed.qr, code: sealed.code });
      for (const name of section.querySelectorAll("[data-device-name]")) name.textContent = pairDevice.name;
      if (form) form.hidden = false;
    } catch (error) {
      console.error(error);
      const box = section.querySelector("[data-pair-open-problem]");
      if (box)
        box.textContent =
          "This code doesn't open the TV's details, so it isn't shown. Check the code on the TV and enter it again.";
    }
  }

  const find = document.querySelector<HTMLFormElement>('form[data-velio="pair-find"]');
  const fragment = storage.get();
  if (find && opened && fragment && !find.dataset.filled) {
    find.dataset.filled = "yes";
    const input = find.querySelector<HTMLInputElement>('[data-field="code"]');
    if (input) input.value = fragment;
    // After HTMX has taken in the swapped form.
    setTimeout(() => find.requestSubmit(), 0);
  }
};

document.addEventListener("htmx:confirm", event => {
  const detail = readConfirm(event);
  const kind = detail?.form.dataset.velio;
  if (!detail || kind === undefined) return;
  // Own keys only, so a name such as "toString" isn't taken for a handler.
  const edit = Object.prototype.hasOwnProperty.call(edits, kind) ? edits[kind] : undefined;
  const credential = Object.prototype.hasOwnProperty.call(credentials, kind) ? credentials[kind] : undefined;
  const pairing = kind === "pair-find" || kind === "pair";
  if (!credential && !edit && !pairing) return;
  // Without this, HTMX would show window.confirm().
  event.preventDefault();
  if (!detail.body) detail.dropRequest();
  else if (credential) void sendCredentials(credential, detail, detail.body);
  else if (kind === "pair-find") findPairing(detail, detail.body);
  else if (kind === "pair") sendPairing(detail, detail.body);
  else if (edit) sendChange(edit, detail, detail.body);
});

// --- Showing pages ---

const KIND_TEXT = { m3u: "M3U playlist", xtream: "Xtream login", stremio: "Stremio add-on" };

const hostOf = (address: string | undefined) => {
  if (!address) return "";
  try {
    return new URL(/^[a-z]+:\/\//i.test(address) ? address : `http://${address}`).hostname;
  } catch {
    return "";
  }
};

/** What a list is, without its address's path or query, which can hold a login. */
const summaryOf = (list: OpenedList) => {
  const { record } = list;
  const parts = [KIND_TEXT[record.kind], hostOf(record.kind === "xtream" ? record.server : record.url)];
  if (record.disabled) parts.push("turned off");
  return parts.filter(Boolean).join(" · ");
};

const fillText = (attribute: string, texts: Map<string, string>) => {
  for (const element of document.querySelectorAll<HTMLElement>(`[${attribute}]`)) {
    const text = texts.get(element.getAttribute(attribute) ?? "");
    if (text !== undefined) element.textContent = text;
  }
};

const showKind = (form: HTMLFormElement) => {
  const kind = valueOf(form, '[data-field="kind"]');
  for (const fieldset of form.querySelectorAll<HTMLElement>("[data-kind-only]"))
    fieldset.hidden = fieldset.dataset.kindOnly !== kind;
  for (const fieldset of form.querySelectorAll<HTMLElement>("[data-kind-not]"))
    fieldset.hidden = fieldset.dataset.kindNot === kind;
};

/** Fills a form once, so a problem shown after sending doesn't undo what was typed. */
const fillOnce = (form: HTMLFormElement, fill: (set: (field: string, value: string) => void) => void) => {
  if (form.dataset.filled) return;
  form.dataset.filled = "yes";
  fill((field, value) => {
    const input = form.querySelector<HTMLInputElement | HTMLSelectElement | HTMLTextAreaElement>(
      `[data-field="${field}"]`,
    );
    if (input) input.value = value;
  });
};

const fill = (state: OpenedState) => {
  fillText(
    "data-profile-name",
    new Map(state.profiles.map(profile => [profile.record.id, profile.record.name])),
  );
  fillText("data-list-name", new Map(state.lists.map(list => [list.record.id, list.record.name])));
  fillText("data-list-summary", new Map(state.lists.map(list => [list.record.id, summaryOf(list)])));
  // A TV from before names were sealed has none.
  fillText("data-tv-name", new Map(state.devices.map(device => [device.id, device.name ?? "TV"])));

  drawProfilePictures(state);
  for (const form of document.querySelectorAll<HTMLFormElement>('form[data-velio="profile-edit"]')) {
    const profile = state.profiles.find(item => item.record.id === form.dataset.profileId);
    if (!profile) continue;
    const { record } = profile;
    fillOnce(form, set => {
      set("name", record.name);
      set("pin", record.pin ?? "");
      const color = form.querySelector<HTMLInputElement>(
        `[data-color-choice][value="${profileColor(record)}"]`,
      );
      if (color) color.checked = true;
    });
  }

  const listForm = document.querySelector<HTMLFormElement>("form[data-list-form]");
  const list = state.lists.find(item => item.record.id === listForm?.dataset.listId);
  if (listForm && list) {
    const { record } = list;
    fillOnce(listForm, set => {
      set("kind", record.kind);
      set("name", record.name);
      set("url", record.kind === "m3u" ? (record.url ?? "") : "");
      set("addonUrl", record.kind === "stremio" ? (record.url ?? "") : "");
      set("server", record.server ?? "");
      set("username", record.username ?? "");
      set("password", record.password ?? "");
      set("guideUrls", (record.guideUrls ?? []).join("\n"));
      set("refreshHours", String(record.refreshHours));
      const disabled = listForm.querySelector<HTMLInputElement>('[data-field="disabled"]');
      if (disabled) disabled.checked = record.disabled === true;
    });
  }
  if (listForm) showKind(listForm);
};

/** Profile pictures and the colour choices, coloured through the CSSOM, which style-src 'self' allows. */
const drawProfilePictures = (state: OpenedState) => {
  for (const swatch of document.querySelectorAll<HTMLElement>("[data-swatch]")) {
    const color = PROFILE_COLORS[Number(swatch.dataset.swatch)];
    if (color) swatch.style.backgroundColor = color.background;
  }
  for (const pic of document.querySelectorAll<HTMLElement>("[data-profile-pic]")) {
    const profile = state.profiles.find(item => item.record.id === pic.dataset.profilePic);
    if (!profile) continue;
    pic.textContent = profileInitial(profile.record.name);
    pic.style.backgroundColor = PROFILE_COLORS[profileColor(profile.record)]?.background ?? "";
    pic.style.color = PROFILE_LETTER_COLOR;
  }
  for (const note of document.querySelectorAll<HTMLElement>("[data-profile-pin-note]")) {
    const profile = state.profiles.find(item => item.record.id === note.dataset.profilePinNote);
    note.textContent = profile?.record.pin === undefined ? "" : " · PIN set";
  }
};

const setText = (selector: string, text: string) => {
  for (const element of document.querySelectorAll(selector)) element.textContent = text;
};

// --- The owner's copy of their lists ---

/** Saves the opened lists as a JSON file; nothing is sent anywhere. */
const downloadLists = (state: OpenedState) => {
  const now = new Date();
  const file = new Blob([`${JSON.stringify(exportLists(state, now), null, 2)}\n`], {
    type: "application/json",
  });
  const link = document.createElement("a");
  link.href = URL.createObjectURL(file);
  link.download = exportFileName(now);
  document.body.append(link);
  link.click();
  link.remove();
  setTimeout(() => URL.revokeObjectURL(link.href), 60_000);
};

// --- Rollback ---
//
// A server could send an older signed state of the account, such as one from
// before a TV was removed; an edit made on it would sign that TV back in.
// This browser remembers the newest revision it has opened for the
// account's signing key, and refuses older ones. A browser that never saw
// the newer revision can't tell (PLAN.md, "Phase 7 results").

const SEEN_STORAGE = "velio-seen";

type Seen = Record<string, unknown>;

const readSeen = (): Seen => {
  try {
    const value: unknown = JSON.parse(localStorage.getItem(SEEN_STORAGE) ?? "{}");
    return isRecord(value) ? value : {};
  } catch {
    return {};
  }
};

/** The revision this browser has seen for the account's keys, if newer than `rev`; otherwise records `rev`. */
const newerSeen = (accountId: string, signPublicKey: string, rev: number) => {
  const seen = readSeen();
  const entry = seen[accountId];
  const known =
    isRecord(entry) && entry.signPublicKey === signPublicKey && typeof entry.rev === "number"
      ? entry.rev
      : -1;
  if (known > rev) return known;
  if (known < rev) {
    seen[accountId] = { signPublicKey, rev };
    try {
      localStorage.setItem(SEEN_STORAGE, JSON.stringify(seen));
    } catch {
      // Without storage, the check only holds within this tab's pages.
    }
  }
  return undefined;
};

/** Opens an owner bundle that came with a sign-in or an unlock. */
const openBundle = (holder: HTMLElement, accountId: string | null) => {
  const unlockKey = pendingUnlockKey;
  pendingUnlockKey = null;
  holder.remove();
  if (!unlockKey || !accountId) return false;
  try {
    const sealed: unknown = JSON.parse(holder.dataset.ownerBundle ?? "");
    if (!isSealed(sealed)) throw new Error("The owner bundle isn't readable");
    owner = openOwnerBundle(unlockKey, sealed, {
      signPublicKey: holder.dataset.signPublicKey ?? "",
      boxPublicKey: holder.dataset.boxPublicKey ?? "",
    });
    ownerAccount = accountId;
    // Shown as remembered once it's stored.
    if (pendingRemember)
      void remember(accountId, owner).then(render, (error: unknown) => console.error(error));
    else void forget();
    return true;
  } catch (error) {
    console.error(error);
    lock();
    return false;
  }
};

/** A recovery key just made, shown once where the answer says. */
const showRecoveryKey = () => {
  const code = document.querySelector<HTMLElement>("[data-recovery-code]");
  if (!code || code.dataset.shown) return;
  code.dataset.shown = "yes";
  code.textContent = pendingRecoveryKey
    ? formatRecoveryKey(pendingRecoveryKey)
    : "This tab no longer has it. Make a new one.";
  pendingRecoveryKey = null;
};

/** After each swap and on load: open what the page holds, and show or hide what needs the keys. */
const render = () => {
  const accountId = pageAccount();
  if (!accountId) {
    if (owner || remembered) lock();
    else void forget();
  } else if (ownerAccount !== accountId) {
    owner = null;
    ownerAccount = null;
  }

  const bundle = document.querySelector<HTMLElement>("[data-owner-bundle]");
  const bundleFailed = bundle ? !openBundle(bundle, accountId) : false;

  const holder = document.querySelector<HTMLElement>("[data-vault-state]");
  // Keys remembered from before a reset of the account sign nothing it trusts.
  if (owner && holder && holder.dataset.signPublicKey !== toBase64(owner.signing.publicKey)) lock();

  let problem = "";
  opened = null;
  if (owner && holder && accountId) {
    try {
      opened = openState(owner, accountId, readStoredState(JSON.parse(holder.dataset.vaultState ?? "")));
      const { rev } = opened.manifest;
      const newer = newerSeen(accountId, toBase64(owner.signing.publicKey), rev);
      if (newer !== undefined) {
        opened = null;
        problem = `Velio sent an older version of your account (revision ${rev}) than this browser has already opened (revision ${newer}), so it isn't shown and can't be changed. Reload the page; if this keeps happening, tell us.`;
      }
    } catch (error) {
      console.error(error);
      problem =
        "What Velio sent for this account didn't pass this browser's checks, so it isn't shown. Reload the page; if this keeps happening, tell us.";
    }
  }

  const open = owner !== null && (holder === null || opened !== null);
  for (const element of document.querySelectorAll<HTMLElement>("[data-locked]")) element.hidden = open;
  for (const element of document.querySelectorAll<HTMLElement>("[data-needs-keys]")) element.hidden = !open;
  for (const element of document.querySelectorAll<HTMLElement>("[data-vault-problem]")) {
    element.textContent = problem;
    element.hidden = problem === "";
  }
  if (bundleFailed)
    setText(
      "[data-locked] [data-vault-status]",
      "Signed in, but your keys didn't open. Reset the account if this keeps happening.",
    );
  if (open)
    setText(
      "[data-needs-keys] [data-vault-status]",
      remembered
        ? "Your keys are open, and this browser remembers them until you sign out or lock it."
        : "Your keys are open in this tab. Reloading the page locks them again.",
    );
  if (opened) fill(opened);
  showPairing();
  showRecoveryKey();

  // A Turnstile token works once; a new one for the next try.
  if (window.turnstile && document.querySelector(".cf-turnstile")) window.turnstile.reset();
};

// Turnstile keeps checking on its widgets; one that a swap took out of the
// page is removed, so it stops (and stops warning in the console).
let widgets: Element[] = [];
document.addEventListener("htmx:before:swap", () => {
  widgets = [...document.querySelectorAll(".cf-turnstile")];
});
const removeGoneWidgets = () => {
  for (const widget of widgets)
    if (!widget.isConnected)
      try {
        window.turnstile?.remove(widget);
      } catch {
        // Already gone.
      }
  widgets = [];
};

document.addEventListener("htmx:after:swap", () => {
  removeGoneWidgets();
  render();
});
document.addEventListener("htmx:response:error", () => {
  pendingUnlockKey = null;
  pendingRecoveryKey = null;
});

document.addEventListener("change", event => {
  const target = event.target;
  if (target instanceof HTMLSelectElement && target.dataset.field === "kind" && target.form)
    showKind(target.form);
});

document.addEventListener("click", event => {
  if (!(event.target instanceof Element)) return;
  if (event.target.closest("[data-forget]")) {
    lock();
    render();
  }
  if (event.target.closest("[data-print]")) window.print();
  if (event.target.closest("[data-export]") && opened) downloadLists(opened);
});

/** On load: keys this browser remembers, then the page. */
const start = async () => {
  capturePairCode();
  const accountId = pageAccount();
  if (accountId) {
    try {
      const keys = await recall(accountId);
      if (keys) {
        owner = keys;
        ownerAccount = accountId;
        remembered = true;
      }
    } catch (error) {
      console.error(error);
      await forget();
    }
  }
  render();
};

void start();

/** For later phases and the console: whether this tab holds the owner's keys. */
window.velioVault = { unlocked: () => owner !== null };
