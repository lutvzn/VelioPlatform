// Profiles, add-ons and IPTV lists, saved on the website and read by a TV
// through Nuvio's functions (and Velio's velio_pull_iptv_lists), as the
// signed-in user, so the database's policies decide.

import { describe, expect, it } from "vitest";
import { lit, sql } from "./support/runtime.ts";
import { post, request, rpc, signedInAccount, textOf, type Visitor } from "./helpers.ts";
import { signInTv } from "./tv.ts";

interface Profile {
  profile_index: number;
  name: string;
  avatar_color_hex: string;
  pin_enabled: boolean;
}

interface List {
  id: string;
  name: string;
  kind: string;
  url: string | null;
  server: string | null;
  username: string | null;
  password: string | null;
  guide_urls: string[];
  refresh_hours: number;
  disabled: boolean;
  profile_ids: number[];
}

const profilesOf = async (token: string) => (await rpc("sync_pull_profiles", {}, token)).json<Profile[]>();

const listsFor = async (token: string, profile: number) => {
  const answer = await rpc("velio_pull_iptv_lists", { p_profile_id: profile }, token);
  expect(answer.status).toBe(200);
  return answer.json<List[]>();
};

/** An account starts with no profiles; the first one made is the main one, 1. */
const withMainProfile = async (browser: Visitor) => {
  expect((await post(browser, "/profiles", { name: "Main" })).status).toBe(200);
};

const listIds = (userId: string) =>
  sql<{ id: string; name: string }>(
    `SELECT id, name FROM public.velio_iptv_lists WHERE user_id = ${lit(userId)} ORDER BY sort_order`,
  );

describe("profiles", () => {
  it("are added, changed and deleted on the website, and the TV sees them", async () => {
    const { browser } = await signedInAccount();
    const tv = await signInTv(browser);
    const token = tv.tokens.access_token;
    expect(await profilesOf(token)).toEqual([]);
    await withMainProfile(browser);
    const before = await profilesOf(token);
    expect(before.map(profile => [profile.profile_index, profile.name])).toEqual([[1, "Main"]]);

    const added = await post(browser, "/profiles", { name: "Kids" });
    expect(added.status).toBe(200);
    expect(await textOf(added)).toContain("The profile is added.");
    const kids = (await profilesOf(token)).find(profile => profile.name === "Kids");
    if (!kids) throw new Error("The TV doesn't see the new profile");
    expect((await profilesOf(token)).length).toBe(before.length + 1);

    const changed = await post(browser, `/profiles/${kids.profile_index}`, {
      name: "Children",
      color: "#E53935",
    });
    expect(changed.status).toBe(200);
    const after = (await profilesOf(token)).find(profile => profile.profile_index === kids.profile_index);
    expect(after).toMatchObject({ name: "Children", avatar_color_hex: "#E53935" });

    expect((await post(browser, `/profiles/${kids.profile_index}`, { name: "" })).status).toBe(400);
    expect((await post(browser, "/profiles/1/delete", {})).status).toBe(400);
    expect((await post(browser, `/profiles/${kids.profile_index}/delete`, {})).status).toBe(200);
    expect((await profilesOf(token)).map(profile => profile.profile_index)).not.toContain(kids.profile_index);
  });

  it("take a PIN of 4 digits, which changing or clearing needs", async () => {
    const { browser } = await signedInAccount();
    await withMainProfile(browser);
    await post(browser, "/profiles", { name: "Locked" });
    const page = await textOf(await request(browser, "/profiles"));
    const index = /\/profiles\/(\d+)\/pin/g.exec(page.slice(page.indexOf("Locked")))?.[1];
    if (!index) throw new Error("No PIN form for the profile");

    expect((await post(browser, `/profiles/${index}/pin`, { pin: "12a4" })).status).toBe(400);
    expect((await post(browser, `/profiles/${index}/pin`, { pin: "1234" })).status).toBe(200);
    const wrong = await post(browser, `/profiles/${index}/pin`, { pin: "", current_pin: "0000" });
    expect(wrong.status).toBe(400);
    expect(wrong.headers.get("HX-Retarget")).toBe("#form-problem");
    const cleared = await post(browser, `/profiles/${index}/pin`, { pin: "", current_pin: "1234" });
    expect(cleared.status).toBe(200);
    expect(await textOf(cleared)).toContain("The profile has no PIN now.");
  });

  it("have add-ons, with stremio:// read as https://", async () => {
    const { browser, userId } = await signedInAccount();
    const added = await post(browser, "/profiles/1/addons", { url: "stremio://addon.example/manifest.json" });
    expect(added.status).toBe(200);
    const rows = sql<{ id: string; url: string }>(
      `SELECT id, url FROM public.addons WHERE user_id = ${lit(userId)} AND profile_id = 1 ORDER BY sort_order`,
    );
    const addon = rows.find(row => row.url === "https://addon.example/manifest.json");
    if (!addon) throw new Error("The add-on isn't saved");
    expect((await post(browser, "/profiles/1/addons", { url: "javascript:alert(1)" })).status).toBe(400);
    expect((await post(browser, `/profiles/1/addons/${addon.id}/delete`, {})).status).toBe(200);
    expect(sql(`SELECT 1 FROM public.addons WHERE id = ${lit(addon.id)}`)).toEqual([]);
  });
});

describe("lists", () => {
  it("are added on the website, and read by the TV for each profile", async () => {
    const { browser, userId } = await signedInAccount();
    const tv = await signInTv(browser);
    await withMainProfile(browser);
    await post(browser, "/profiles", { name: "Kids" });
    const kids = (await profilesOf(tv.tokens.access_token)).find(profile => profile.name === "Kids");
    if (!kids) throw new Error("No profile");

    const m3u = await post(browser, "/lists", {
      kind: "m3u",
      url: "https://lists.example/home.m3u",
      name: "Home",
      guide_urls: "https://lists.example/guide.xml\nhttps://lists.example/more.xml",
      refresh_hours: "12",
    });
    expect(m3u.status).toBe(200);
    expect(await textOf(m3u)).toContain("The list is added.");
    const body = new URLSearchParams({
      kind: "xtream",
      server: "tv.example:8080",
      username: "someone",
      password: " a secret ",
      refresh_hours: "24",
    });
    body.append("profile", String(kids.profile_index));
    const xtream = await request(browser, "/lists", {
      method: "POST",
      headers: {
        Origin: "https://velio.tv",
        "HX-Request": "true",
        "Content-Type": "application/x-www-form-urlencoded",
      },
      body,
    });
    expect(xtream.status).toBe(200);

    const forMain = await listsFor(tv.tokens.access_token, 1);
    expect(forMain).toHaveLength(1);
    expect(forMain[0]).toMatchObject({
      name: "Home",
      kind: "m3u",
      url: "https://lists.example/home.m3u",
      guide_urls: ["https://lists.example/guide.xml", "https://lists.example/more.xml"],
      refresh_hours: 12,
      disabled: false,
      profile_ids: [],
    });
    const forKids = await listsFor(tv.tokens.access_token, kids.profile_index);
    expect(forKids.map(list => list.kind)).toEqual(["m3u", "xtream"]);
    // A server with no scheme is http, the name its host, and the password as typed.
    expect(forKids[1]).toMatchObject({
      name: "tv.example",
      server: "http://tv.example:8080",
      username: "someone",
      password: " a secret ",
      profile_ids: [kids.profile_index],
    });
    expect(listIds(userId).map(list => list.name)).toEqual(["Home", "tv.example"]);
  });

  it("are changed and deleted, and refuse what isn't a list", async () => {
    const { browser, userId } = await signedInAccount();
    await post(browser, "/lists", { kind: "m3u", url: "https://lists.example/a.m3u", refresh_hours: "24" });
    const [list] = listIds(userId);
    if (!list) throw new Error("No list");
    expect(list.name).toBe("lists.example");

    const page = await request(browser, `/lists/${list.id}`);
    expect(await textOf(page)).toContain('value="https://lists.example/a.m3u"');
    const saved = await post(browser, `/lists/${list.id}`, {
      kind: "m3u",
      url: "https://lists.example/b.m3u",
      name: "Renamed",
      refresh_hours: "168",
      disabled: "yes",
    });
    expect(saved.status).toBe(200);
    expect(
      sql(
        `SELECT name, url, refresh_hours, disabled FROM public.velio_iptv_lists WHERE id = ${lit(list.id)}`,
      ),
    ).toEqual([{ name: "Renamed", url: "https://lists.example/b.m3u", refresh_hours: 168, disabled: true }]);

    const refusals: Record<string, string>[] = [
      { kind: "m3u", url: "not a url", refresh_hours: "24" },
      { kind: "m3u", url: "ftp://lists.example/a.m3u", refresh_hours: "24" },
      { kind: "xtream", server: "tv.example", username: "", password: "x", refresh_hours: "24" },
      { kind: "other", url: "https://lists.example/a.m3u", refresh_hours: "24" },
    ];
    for (const fields of refusals) {
      const refused = await post(browser, "/lists", fields);
      expect(refused.status).toBe(400);
      expect(refused.headers.get("HX-Retarget")).toBe("#form-problem");
    }

    expect((await post(browser, `/lists/${list.id}/delete`, {})).status).toBe(200);
    expect(listIds(userId)).toEqual([]);
  });

  it("are the account's own: another account can't open or change them", async () => {
    const owner = await signedInAccount();
    const other = await signedInAccount();
    await post(owner.browser, "/lists", {
      kind: "m3u",
      url: "https://lists.example/mine.m3u",
      refresh_hours: "24",
    });
    const [list] = listIds(owner.userId);
    if (!list) throw new Error("No list");
    expect((await request(other.browser, `/lists/${list.id}`)).status).toBe(404);
    await post(other.browser, `/lists/${list.id}/delete`, {});
    expect(listIds(owner.userId)).toHaveLength(1);
    // The website needs a session.
    expect((await request({ ip: owner.browser.ip }, "/lists")).status).toBe(302);
  });
});
