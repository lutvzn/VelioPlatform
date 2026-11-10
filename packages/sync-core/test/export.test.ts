import { describe, expect, it } from "vitest";
import { openState, withList, withProfile, type StoredState } from "../src/changes.ts";
import { exportFileName, exportLists } from "../src/export.ts";
import { newOwnerKeys } from "../src/owner.ts";

const NOTHING: StoredState = { manifest: null, profiles: [], lists: [], devices: [] };

describe("exporting the lists", () => {
  it("gives every list's settings, with its profiles by name, and the profiles", () => {
    let state = openState(newOwnerKeys(), "account-1", NOTHING);
    const kids = withProfile(state, { name: "Kids", color: 3, pin: "0042" });
    state = kids.state;
    const grown = withProfile(state, { name: "Grown-ups" });
    state = grown.state;
    state = withList(
      state,
      {
        name: "Sports",
        kind: "xtream",
        server: "http://x.example.invalid",
        username: "me",
        password: "secret",
        refreshHours: 12,
      },
      [kids.profileId, grown.profileId],
    ).state;
    const made = withList(
      state,
      { name: "News", kind: "m3u", url: "http://n.example.invalid/a.m3u", refreshHours: 24 },
      [],
    );
    state = made.state;

    const now = new Date("2026-11-10T12:00:00Z");
    const exported = exportLists(state, now);
    expect(exported).toMatchObject({ velio: "lists", format: 1, exportedAt: "2026-11-10T12:00:00.000Z" });
    expect(exported.profiles).toEqual([
      { name: "Kids", color: 3, pin: "0042" },
      { name: "Grown-ups", color: expect.any(Number) },
    ]);
    expect(exported.lists).toEqual([
      {
        id: expect.any(String),
        name: "Sports",
        kind: "xtream",
        server: "http://x.example.invalid",
        username: "me",
        password: "secret",
        refreshHours: 12,
        profiles: ["Kids", "Grown-ups"],
      },
      {
        id: made.listId,
        name: "News",
        kind: "m3u",
        url: "http://n.example.invalid/a.m3u",
        refreshHours: 24,
        profiles: [],
      },
    ]);
    expect(exportFileName(now)).toBe("velio-lists-2026-11-10.json");
  });
});
