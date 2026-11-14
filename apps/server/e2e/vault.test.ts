// Phase 2, end to end (PLAN.md, Phases: "Done when"):
// lists made in one browser show in another; the database holds only
// ciphertext; a taken list's old key opens no new version.
//
// The tests stand in for vault.js: they read [data-vault-state] from pages,
// open it with sync-core, and send changes built with its builders.

import {
  buildChange,
  newOwnerKeys,
  openJson,
  openState,
  newKey,
  toBase64,
  unwrapKey,
  withList,
  withProfile,
  type ListDraft,
} from "@velio/sync-core";
import { describe, expect, it } from "vitest";
import { accountStub, everythingStored } from "./support/account.ts";
import { env } from "./support/runtime.ts";
import { openPage, post, request, save, send, signedInAccount, stateIn, textOf } from "./helpers.ts";

const M3U: ListDraft = {
  name: "Home playlist",
  kind: "m3u",
  url: "http://playlist.example.invalid/get.php?username=secret-user&password=secret-pass",
  refreshHours: 24,
};

const XTREAM: ListDraft = {
  name: "Xtream at home",
  kind: "xtream",
  server: "http://xtream.example.invalid:8080",
  username: "someone-secret",
  password: "hunter2-but-longer",
  refreshHours: 12,
};

describe("the pages", () => {
  it("need a session", async () => {
    for (const path of ["/lists", "/profiles", "/lists/new"]) {
      const page = await request({ ip: "192.0.2.1" }, path);
      expect(page.status).toBe(302);
      expect(page.headers.get("Location")).toBe("/signin");
    }
    const change = await post({ ip: "192.0.2.1" }, "/changes", { change: "{}", next: "/lists" });
    expect(change.status).toBe(401);
  });

  it("show the shape and the ciphertext, and nothing readable", async () => {
    const { browser } = await signedInAccount();
    let profileId = "";
    await save(browser, state => {
      const made = withProfile(state, { name: "Living room" });
      profileId = made.profileId;
      return withList(made.state, M3U, [profileId]).state;
    });
    const { html, state } = await openPage(browser);
    expect(state.lists.map(list => list.record.name)).toEqual(["Home playlist"]);
    expect(html).toContain(`data-profile-name="${profileId}"`);
    expect(html).toContain(`data-list-name="${state.lists[0].record.id}"`);
    for (const secret of ["Home playlist", "Living room", "secret-user", "playlist.example.invalid"])
      expect(html).not.toContain(secret);

    const listPage = await request(browser.visitor, `/lists/${state.lists[0].record.id}`);
    expect(listPage.status).toBe(200);
    expect(await listPage.text()).toMatch(/data-profile-choice="[^"]+" checked/);
    expect((await request(browser.visitor, "/lists/nothing-here")).status).toBe(404);
  });
});

describe("changes", () => {
  it("made in one browser show in another", async () => {
    const { browser, another } = await signedInAccount();
    const answer = await save(browser, state => {
      const made = withProfile(state, { name: "Kids" });
      return withList(withList(made.state, M3U, [made.profileId]).state, XTREAM, []).state;
    });
    expect(answer.headers.get("HX-Push-Url")).toBe("/lists");
    // The answer is the next page, with the account after the change.
    const answered = openState(browser.owner, browser.accountId, stateIn(await answer.text()));
    expect(answered.lists).toHaveLength(2);

    const second = await another();
    const { state } = await openPage(second);
    expect(state.profiles.map(profile => profile.record.name)).toEqual(["Kids"]);
    expect(state.lists.map(list => list.record)).toMatchObject([
      { ...M3U, rev: 1 },
      { ...XTREAM, rev: 1 },
    ]);
    expect(state.lists[0].profiles).toEqual([state.profiles[0].record.id]);
    expect(state.lists[1].profiles).toEqual([]);

    // And the other way.
    await save(
      second,
      current => withProfile(current, { name: "Kids, renamed" }, current.profiles[0].record.id).state,
      "/profiles",
    );
    const { state: back } = await openPage(browser);
    expect(back.profiles[0].record.name).toBe("Kids, renamed");
  });

  it("leave only ciphertext in the database", async () => {
    const { browser, email } = await signedInAccount();
    await save(browser, state => {
      const made = withProfile(state, { name: "Grandma's room" });
      return withList(withList(made.state, M3U, [made.profileId]).state, XTREAM, [made.profileId]).state;
    });
    const stored = everythingStored(browser.accountId);
    expect(stored).toContain("nonce");
    const d1 = JSON.stringify(
      await Promise.all(
        ["accounts", "sessions", "email_links", "dev_outbox"].map(
          async table => (await env.DB.prepare(`SELECT * FROM ${table}`).all()).results,
        ),
      ),
    );
    for (const secret of [
      "Home playlist",
      "Xtream at home",
      "Grandma",
      "playlist.example.invalid",
      "xtream.example.invalid",
      "secret-user",
      "secret-pass",
      "someone-secret",
      "hunter2",
    ]) {
      expect(stored).not.toContain(secret);
      expect(d1).not.toContain(secret);
    }
    expect(stored).not.toContain(email);
  });

  it("answer 409 when another tab changed the account first", async () => {
    const { browser, another } = await signedInAccount();
    const { state } = await openPage(browser);
    const second = await another();
    const { state: secondState } = await openPage(second);
    expect(
      (
        await send(
          second,
          buildChange(second.owner, secondState, withProfile(secondState, { name: "First" }).state),
        )
      ).status,
    ).toBe(200);

    const late = await send(
      browser,
      buildChange(browser.owner, state, withProfile(state, { name: "Second" }).state),
    );
    expect(late.status).toBe(409);
    expect(late.headers.get("HX-Retarget")).toBe("#form-problem");
    expect(await textOf(late)).toContain("changed in another tab");
    // Sending the same change twice is the same.
    const again = await send(
      second,
      buildChange(second.owner, secondState, withProfile(secondState, { name: "First" }).state),
    );
    expect(again.status).toBe(409);
  });

  it("refuse a change the owner didn't sign", async () => {
    const { browser } = await signedInAccount();
    const { state } = await openPage(browser);
    const forged = buildChange(newOwnerKeys(), state, withProfile(state, { name: "Not the owner" }).state);
    expect((await send(browser, forged)).status).toBe(400);
    // Or one that's signed, but with records the manifest doesn't name.
    const real = buildChange(browser.owner, state, withProfile(state, { name: "Owner" }).state);
    expect((await send(browser, { ...real, putProfiles: [] })).status).toBe(400);
    expect((await post(browser.visitor, "/changes", { change: "not json", next: "/lists" })).status).toBe(
      400,
    );
    expect((await openPage(browser)).state.profiles).toEqual([]);
  });

  it("go on only to the account's own pages", async () => {
    const { browser } = await signedInAccount();
    const answer = await save(
      browser,
      state => withProfile(state, { name: "One" }).state,
      "https://example.invalid/",
    );
    expect(answer.headers.get("HX-Push-Url")).toBe("/lists");
    const profiles = await save(browser, state => withProfile(state, { name: "Two" }).state, "/profiles");
    expect(profiles.headers.get("HX-Push-Url")).toBe("/profiles");
  });

  it("give a taken list a new key, which the old one doesn't open", async () => {
    const { browser } = await signedInAccount();
    let kept = "";
    let taken = "";
    await save(browser, state => {
      const first = withProfile(state, { name: "Kept" });
      const second = withProfile(first.state, { name: "Taken" });
      kept = first.profileId;
      taken = second.profileId;
      return withList(second.state, M3U, [kept, taken]).state;
    });
    // What a TV with only the taken profile holds: that profile's key, and the list key under it.
    const before = await openPage(browser);
    const list = before.state.lists[0];
    const tvProfileKey = before.state.profiles.find(profile => profile.record.id === taken)?.key;
    if (!tvProfileKey) throw new Error("No profile key");
    const storedBefore = stateIn(before.html).lists[0];
    const tvListKey = unwrapKey(
      tvProfileKey,
      storedBefore.profileKeys.find(entry => entry.profile === taken)?.key ?? storedBefore.key,
    );
    expect(openJson(tvListKey, storedBefore.sealed)).toMatchObject({ name: "Home playlist" });

    await save(browser, state => withList(state, { ...M3U, name: "Renamed" }, [kept], list.record.id).state);
    const after = await openPage(browser);
    const storedAfter = stateIn(after.html).lists[0];
    expect(storedAfter.keyRev).toBe(2);
    expect(storedAfter.profileKeys.map(entry => entry.profile)).toEqual([kept]);
    expect(() => openJson(tvListKey, storedAfter.sealed)).toThrow(/Couldn't decrypt/);
    expect(after.state.lists[0].record.name).toBe("Renamed");
  });

  it("are deleted with the account's keys on a reset", async () => {
    const { browser } = await signedInAccount();
    await save(browser, state => withProfile(state, { name: "Gone soon" }).state);
    await accountStub(env, browser.accountId).reset();
    const stored = await accountStub(env, browser.accountId).getState();
    expect(stored).toEqual({ manifest: null, profiles: [], lists: [], devices: [] });
  });
});

describe("unlocking", () => {
  it("answers the owner bundle for the right key, without a new session", async () => {
    const { browser, email, credentials } = await signedInAccount();
    const answer = await post(browser.visitor, "/unlock", { email, auth_key: credentials.authKey });
    expect(answer.status).toBe(200);
    expect(answer.headers.getSetCookie()).toEqual([]);
    expect(await answer.text()).toContain("data-owner-bundle");

    const wrong = await post(browser.visitor, "/unlock", { email, auth_key: toBase64(newKey()) });
    expect(wrong.status).toBe(401);
    expect(wrong.headers.get("HX-Retarget")).toBe("#unlock-problem");

    const signedOut = await post({ ip: browser.visitor.ip }, "/unlock", {
      email,
      auth_key: credentials.authKey,
    });
    expect(signedOut.status).toBe(401);
  });
});

describe("profile pictures and PINs (Phase 4)", () => {
  it("are saved sealed and show in another browser", async () => {
    const { browser, another } = await signedInAccount();
    await save(browser, state => withProfile(state, { name: "Kids" }).state, "/profiles");
    const { html, state } = await openPage(browser, "/profiles");
    const kids = state.profiles[0].record;
    // The first colour, and no PIN, until the owner picks them.
    expect(kids).toMatchObject({ color: 0 });
    expect(kids).not.toHaveProperty("pin");
    // The page has the slots vault.js fills, and every colour to choose from.
    expect(html).toContain(`data-profile-pic="${kids.id}"`);
    expect(html).toContain(`data-velio="profile-edit"`);
    expect(html.match(/data-color-choice/g)).toHaveLength(8);

    await save(
      browser,
      current => withProfile(current, { name: "Kids", color: 4, pin: "9753" }, kids.id).state,
    );
    const second = await another();
    const { state: seen } = await openPage(second, "/profiles");
    expect(seen.profiles[0].record).toMatchObject({ name: "Kids", color: 4, pin: "9753", rev: 2 });
    const stored = everythingStored(browser.accountId);
    expect(stored).not.toContain("9753");
  });
});
