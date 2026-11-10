// The routes for profiles and lists (PLAN.md, "Routes"; Phase 2). Pages
// carry the account's ciphertext; vault.js opens it and sends changes,
// sealed and signed, to POST /changes. The answer is the next page, built
// from what the account holds after the change.

import { Hono } from "hono";
import { ID_PATTERN } from "@velio/sync-core";
import { accountStub } from "../account.ts";
import { authHashOf, findAccountById } from "../lib/accounts.ts";
import { clientIp, readForm, type AppContext, type AppEnv } from "../lib/http.ts";
import { withinLimit } from "../lib/limits.ts";
import { signedIn } from "../lib/session.ts";
import { sameDigest } from "../lib/tokens.ts";
import { forgetDevices, NO_KEYS, refusalText, vaultProps } from "../lib/vault.ts";
import {
  DevicesPage,
  ListPage,
  ListsPage,
  OwnerBundle,
  ProfilesPage,
  type VaultPageProps,
} from "../views/vault.tsx";
import { fullPage, isHtmx, problem, TOO_MANY } from "./web.tsx";

export const vault = new Hono<AppEnv>();

/** 50 lists of at most 48 KB each, plus keys and the manifest. */
const MAX_CHANGE_LENGTH = 3_000_000;

type Page = { title: string; render: (props: VaultPageProps) => ReturnType<typeof ListsPage> };

const LISTS: Page = { title: "Lists", render: ListsPage };

/** The pages a change can go on to; a list's own page only while the list exists. */
const pageFor = (path: string, props: VaultPageProps): Page | null => {
  if (path === "/profiles") return { title: "Profiles", render: ProfilesPage };
  if (path === "/devices") return { title: "TVs", render: DevicesPage };
  if (path === "/lists") return LISTS;
  if (path === "/lists/new") return { title: "Add a list", render: p => <ListPage {...p} /> };
  const listId = /^\/lists\/([^/]+)$/.exec(path)?.[1];
  if (listId && ID_PATTERN.test(listId) && props.manifest.lists.some(list => list.id === listId))
    return { title: "Change the list", render: p => <ListPage {...p} listId={listId} /> };
  return null;
};

const showPage = async (c: AppContext) => {
  const session = signedIn(c);
  if (!session) return c.redirect("/signin");
  const props = await vaultProps(c, session);
  if (!props) return problem(c, NO_KEYS, 500);
  const page = pageFor(c.req.path, props);
  if (!page) return c.notFound();
  return fullPage(c, page.title, page.render(props));
};

vault.get("/profiles", showPage);
vault.get("/devices", showPage);
vault.get("/lists", showPage);
vault.get("/lists/new", showPage);
vault.get("/lists/:id", showPage);

vault.post("/changes", async c => {
  const session = signedIn(c);
  if (!session) return problem(c, "You were signed out. Sign in again, then make the change again.", 401);
  const field = await readForm(c);
  const text = field("change");
  if (text.length > MAX_CHANGE_LENGTH) return problem(c, "This change is too big.", 413);
  let input: unknown;
  try {
    input = JSON.parse(text);
  } catch {
    return problem(c, "This browser didn't send a readable change. Reload the page and try again.");
  }

  const result = await accountStub(c.env, session.accountId).change(session.accountId, input);
  if (!result.ok) {
    if (result.code === "refused")
      console.warn(`Refused a change for ${session.accountId}: ${result.message}`);
    return problem(c, refusalText(result), result.code === "behind" ? 409 : 400);
  }
  await forgetDevices(c.env.DB, session.accountId, result.removedDevices);

  const props = await vaultProps(c, session);
  if (!props) return problem(c, NO_KEYS, 500);
  // The page the form asked for, or the lists, such as after deleting the list being changed.
  const asked = field("next");
  const page = pageFor(asked, props) ?? LISTS;
  const path = page === LISTS ? "/lists" : asked;
  if (!isHtmx(c)) return c.redirect(path, 303);
  c.header("HX-Push-Url", path);
  return c.html(page.render(props));
});

/**
 * Opens a signed-in tab's keys again: the same check as signing in, without
 * a new session. Answers the owner bundle, for vault.js to open.
 */
vault.post("/unlock", async c => {
  const session = signedIn(c);
  if (!session) return problem(c, "You were signed out. Sign in again.", 401, "#unlock-problem");
  if (!(await withinLimit(c.env, "AUTH_LIMIT", `unlock:${clientIp(c)}`)))
    return problem(c, TOO_MANY, 429, "#unlock-problem");
  if (!(await withinLimit(c.env, "EMAIL_LIMIT", `signin:${session.email}`)))
    return problem(c, TOO_MANY, 429, "#unlock-problem");
  const field = await readForm(c);
  const authHash = await authHashOf(field("auth_key"));
  if (!authHash)
    return problem(
      c,
      "This browser didn't send a key. Reload the page and try again.",
      400,
      "#unlock-problem",
    );
  const account = await findAccountById(c.env.DB, session.accountId);
  if (!account || !sameDigest(account.auth_hash, authHash))
    return problem(c, "That isn't the password for this account.", 401, "#unlock-problem");
  const owner = await accountStub(c.env, session.accountId).getOwner();
  if (!owner) return problem(c, NO_KEYS, 500, "#unlock-problem");
  return c.html(<OwnerBundle owner={owner} />);
});
