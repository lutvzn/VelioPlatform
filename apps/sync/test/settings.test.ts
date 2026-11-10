// Phase 6's account settings in the Workers runtime: changing the password,
// the recovery key, and deleting the account. As in helpers.ts, random keys
// stand in for what Argon2id and HKDF make in vault.js: the server only ever
// sees auth keys and sealed bundles.

import { env } from "cloudflare:test";
import {
  DEFAULT_KDF,
  isSealed,
  newKey,
  newOwnerKeys,
  openOwnerBundle,
  ownerPublicKeys,
  sealOwnerBundle,
  toBase64,
  withList,
  type OwnerKeys,
} from "@velio/sync-core";
import { describe, expect, it } from "vitest";
import { accountStub } from "../src/account.ts";
import {
  linkIn,
  openAnswer,
  openPage,
  post,
  request,
  save,
  sessionCookie,
  signedInAccount,
  signin,
  textOf,
  type Browser,
} from "./helpers.ts";

/** What vault.js sends for a new password over the same owner keys. */
const newPassword = (owner: OwnerKeys) => {
  const authKey = newKey();
  const unlockKey = newKey();
  const publicKeys = ownerPublicKeys(owner);
  const fields = {
    salt: toBase64(newKey().subarray(0, 16)),
    kdf_memory_kib: String(DEFAULT_KDF.memoryKiB),
    kdf_iterations: String(DEFAULT_KDF.iterations),
    kdf_parallelism: String(DEFAULT_KDF.parallelism),
    auth_key: toBase64(authKey),
    bundle: JSON.stringify(sealOwnerBundle(unlockKey, owner)),
    sign_public_key: publicKeys.signPublicKey,
    box_public_key: publicKeys.boxPublicKey,
  };
  return { fields, authKey: fields.auth_key, unlockKey };
};

/** A recovery key's auth and unlock keys, as deriveRecoveryKeys() makes them. */
const newRecovery = (owner: OwnerKeys) => {
  const authKey = toBase64(newKey());
  const unlockKey = newKey();
  return {
    authKey,
    unlockKey,
    fields: {
      recovery_auth_key: authKey,
      recovery_bundle: JSON.stringify(sealOwnerBundle(unlockKey, owner)),
    },
  };
};

const listNames = async (browser: Browser) =>
  (await openPage(browser)).state.lists.map(list => list.record.name);

/** An account with a list, its password's auth key, and a second browser signed in. */
const accountWithList = async () => {
  const account = await signedInAccount();
  await save(
    account.browser,
    state =>
      withList(
        state,
        { name: "Kept", kind: "m3u", url: "http://kept.example.invalid/a.m3u", refreshHours: 24 },
        [],
      ).state,
  );
  return { ...account, authKey: account.credentials.authKey, other: await account.another() };
};

const resetLink = async (browser: Browser, email: string) => {
  expect((await post(browser.visitor, "/reset", { email })).status).toBe(200);
  return linkIn(email, "reset");
};

describe("changing the password", () => {
  it("seals the same keys under the new one, keeps the lists, and signs out other browsers", async () => {
    const { email, browser, other, authKey, credentials } = await accountWithList();
    const next = newPassword(credentials.owner);
    const answer = await post(browser.visitor, "/password", { current_auth_key: authKey, ...next.fields });
    expect(answer.status).toBe(200);
    expect(await textOf(answer)).toContain("Your password is changed");
    // This browser gets a new session; the other one is signed out.
    const changed = { ...browser, visitor: { ...browser.visitor, cookie: sessionCookie(answer) } };
    expect(await listNames(changed)).toEqual(["Kept"]);
    expect((await request(other.visitor, "/account")).status).toBe(302);

    expect((await signin({ ip: browser.visitor.ip }, email, authKey)).status).toBe(401);
    const signedIn = await signin({ ip: browser.visitor.ip }, email, next.authKey);
    expect(signedIn.status).toBe(200);
    const owner = await openAnswer(signedIn, next.unlockKey);
    expect(ownerPublicKeys(owner)).toEqual(ownerPublicKeys(credentials.owner));
  });

  it("needs the current password, the account's own keys, and a session", async () => {
    const { browser, authKey, credentials, other } = await accountWithList();
    const next = newPassword(credentials.owner);
    const wrong = await post(browser.visitor, "/password", {
      current_auth_key: toBase64(newKey()),
      ...next.fields,
    });
    expect(wrong.status).toBe(401);
    expect(await textOf(wrong)).toBe("That isn't your password.");
    expect(wrong.headers.get("HX-Retarget")).toBe("#password-problem");

    const otherKeys = newPassword(newOwnerKeys());
    const swapped = await post(browser.visitor, "/password", {
      current_auth_key: authKey,
      ...otherKeys.fields,
    });
    expect(swapped.status).toBe(400);
    expect(await textOf(swapped)).toContain("aren't the account's keys");

    const signedOut = await post({ ip: browser.visitor.ip }, "/password", {
      current_auth_key: authKey,
      ...next.fields,
    });
    expect(signedOut.status).toBe(401);
    // Nothing changed: the other browser is still signed in.
    expect((await request(other.visitor, "/account")).status).toBe(200);
  });
});

describe("the recovery key", () => {
  it("sets a new password from the reset link and keeps the lists, the keys and the recovery key", async () => {
    const { email, browser, authKey, credentials } = await accountWithList();
    const recovery = newRecovery(credentials.owner);
    const on = await post(browser.visitor, "/recovery", {
      action: "on",
      current_auth_key: authKey,
      ...recovery.fields,
    });
    expect(on.status).toBe(200);
    const page = await on.text();
    expect(page).toContain("data-recovery-code");
    expect(page).toContain("On, made");

    const token = await resetLink(browser, email);
    expect(await (await request({ ip: browser.visitor.ip }, `/reset/${token}`)).text()).toContain(
      "With your recovery key",
    );
    const visitor = { ip: browser.visitor.ip };
    const wrongKey = await post(visitor, `/reset/${token}/recovery-bundle`, {
      recovery_auth_key: toBase64(newKey()),
    });
    expect(wrongKey.status).toBe(401);
    const bundleAnswer = await post(visitor, `/reset/${token}/recovery-bundle`, {
      recovery_auth_key: recovery.authKey,
    });
    expect(bundleAnswer.status).toBe(200);
    const sent = await bundleAnswer.json<{ bundle: unknown; signPublicKey: string; boxPublicKey: string }>();
    if (!isSealed(sent.bundle)) throw new Error("No bundle");
    const opened = openOwnerBundle(recovery.unlockKey, sent.bundle, sent);
    expect(ownerPublicKeys(opened)).toEqual(ownerPublicKeys(credentials.owner));

    const next = newPassword(opened);
    // A wrong key doesn't use the link.
    const refused = await post(visitor, `/reset/${token}/recover`, {
      ...next.fields,
      recovery_auth_key: toBase64(newKey()),
    });
    expect(refused.status).toBe(401);
    expect(refused.headers.get("HX-Retarget")).toBe("#recover-problem");
    const done = await post(visitor, `/reset/${token}/recover`, {
      ...next.fields,
      recovery_auth_key: recovery.authKey,
    });
    expect(done.status).toBe(200);
    const recovered = { ...browser, visitor: { ...visitor, cookie: sessionCookie(done) } };
    expect(await textOf(done.clone())).toContain("Your new password is set");
    expect(ownerPublicKeys(await openAnswer(done, next.unlockKey))).toEqual(
      ownerPublicKeys(credentials.owner),
    );
    expect(await listNames(recovered)).toEqual(["Kept"]);

    // The link worked once; the old password and session no longer do; the recovery key still does.
    expect(
      (await post(visitor, `/reset/${token}/recover`, { ...next.fields, ...recovery.fields })).status,
    ).toBe(404);
    expect((await request(browser.visitor, "/account")).status).toBe(302);
    expect((await signin(visitor, email, authKey)).status).toBe(401);
    expect(await accountStub(env, browser.accountId).recoveryMadeAt()).not.toBeNull();
  });

  it("turns off, and a reset without it deletes it with the keys", async () => {
    const { email, browser, authKey, credentials } = await accountWithList();
    const recovery = newRecovery(credentials.owner);
    await post(browser.visitor, "/recovery", { action: "on", current_auth_key: authKey, ...recovery.fields });
    const wrong = await post(browser.visitor, "/recovery", {
      action: "off",
      current_auth_key: toBase64(newKey()),
    });
    expect(wrong.status).toBe(401);
    expect(wrong.headers.get("HX-Retarget")).toBe("#recovery-off-problem");
    const off = await post(browser.visitor, "/recovery", { action: "off", current_auth_key: authKey });
    expect(off.status).toBe(200);
    expect(await textOf(off)).toContain("The recovery key is off.");
    const token = await resetLink(browser, email);
    expect(await (await request({ ip: browser.visitor.ip }, `/reset/${token}`)).text()).not.toContain(
      "With your recovery key",
    );
    const visitor = { ip: browser.visitor.ip };
    expect(
      (await post(visitor, `/reset/${token}/recovery-bundle`, { recovery_auth_key: recovery.authKey }))
        .status,
    ).toBe(401);

    // On again, then a reset that deletes the lists: the recovery copy goes with them.
    const signedIn = await signin(visitor, email, authKey);
    const again = { ...browser, visitor: { ...visitor, cookie: sessionCookie(signedIn) } };
    await post(again.visitor, "/recovery", { action: "on", current_auth_key: authKey, ...recovery.fields });
    const second = await resetLink(again, email);
    const fresh = newPassword(newOwnerKeys());
    expect((await post(visitor, `/reset/${second}`, fresh.fields)).status).toBe(200);
    expect(await accountStub(env, browser.accountId).recoveryMadeAt()).toBeNull();
  });
});

describe("deleting the account", () => {
  it("needs the password, then deletes everything and signs out", async () => {
    const { email, browser, authKey, other } = await accountWithList();
    const wrong = await post(browser.visitor, "/account/delete", { current_auth_key: toBase64(newKey()) });
    expect(wrong.status).toBe(401);
    expect(wrong.headers.get("HX-Retarget")).toBe("#delete-problem");
    expect(await listNames(browser)).toEqual(["Kept"]);

    const deleted = await post(browser.visitor, "/account/delete", { current_auth_key: authKey });
    expect(deleted.status).toBe(200);
    expect(await textOf(deleted)).toContain("Your account is deleted");
    expect(deleted.headers.getSetCookie().join("\n")).toMatch(/__Host-velio=;/);

    const id = browser.accountId;
    for (const table of ["accounts WHERE id", "sessions WHERE account_id", "email_links WHERE account_id"])
      expect(await env.DB.prepare(`SELECT count(*) AS n FROM ${table} = ?`).bind(id).first("n")).toBe(0);
    const stub = accountStub(env, id);
    expect(await stub.getOwner()).toBeNull();
    expect(await stub.recoveryMadeAt()).toBeNull();
    expect((await request(other.visitor, "/account")).status).toBe(302);
    expect((await signin({ ip: browser.visitor.ip }, email, authKey)).status).toBe(401);
  });
});
