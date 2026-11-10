import { describe, expect, it } from "vitest";
import {
  applyChange,
  buildChange,
  ChangeProblem,
  openState,
  readStoredState,
  withList,
  withoutList,
  withoutProfile,
  withProfile,
  type OpenedState,
  type StoredState,
} from "../src/changes.ts";
import { isRecord, openJson, signJson, unwrapKey } from "../src/crypto.ts";
import { newOwnerKeys, type OwnerKeys } from "../src/owner.ts";
import {
  defaultListName,
  emptyManifest,
  listProblem,
  MAX_LISTS,
  MAX_PROFILES,
  readList,
  type ListDraft,
} from "../src/records.ts";

const ACCOUNT = "account-1";

const M3U: ListDraft = {
  name: "Home playlist",
  kind: "m3u",
  url: "http://m3u.example.invalid/playlist.m3u?token=secret-token",
  guideUrls: ["http://m3u.example.invalid/guide.xml.gz"],
  refreshHours: 24,
};
const XTREAM: ListDraft = {
  name: "",
  kind: "xtream",
  server: "http://xtream.example.invalid:8080",
  username: "someone",
  password: "hunter2-but-longer",
  refreshHours: 12,
};

/** A server and one owner device, talking only through changes and stored state. */
const setup = () => {
  const owner = newOwnerKeys();
  let stored: StoredState = { manifest: null, profiles: [], lists: [], devices: [] };
  const server = (change: unknown) => {
    // Through JSON, as over the network.
    stored = applyChange(stored, JSON.parse(JSON.stringify(change)), ACCOUNT, owner.signing.publicKey).state;
    return stored;
  };
  const open = (keys: OwnerKeys = owner) =>
    openState(keys, ACCOUNT, readStoredState(JSON.parse(JSON.stringify(stored))));
  const save = (edit: (state: OpenedState) => OpenedState) => {
    const before = open();
    server(buildChange(owner, before, edit(before)));
    return open();
  };
  return { owner, server, open, save, stored: () => stored };
};

/** Two profiles, "Living room" and "Kids". */
const withTwoProfiles = () => {
  const account = setup();
  let living = "";
  let kids = "";
  account.save(state => {
    const first = withProfile(state, { name: "Living room" });
    living = first.profileId;
    const second = withProfile(first.state, { name: "Kids" });
    kids = second.profileId;
    return second.state;
  });
  return { ...account, living, kids };
};

describe("an account's lists and profiles", () => {
  it("start empty, at revision 0", () => {
    const { open } = setup();
    expect(open()).toEqual({ manifest: emptyManifest(ACCOUNT), profiles: [], lists: [], devices: [] });
  });

  it("come back from the server as they were saved", () => {
    const { save, living, kids } = withTwoProfiles();
    let listId = "";
    const state = save(before => {
      const added = withList(before, M3U, [living, kids]);
      listId = added.listId;
      return withList(added.state, XTREAM, [living]).state;
    });
    expect(state.manifest.rev).toBe(2);
    expect(state.profiles.map(profile => profile.record.name)).toEqual(["Living room", "Kids"]);
    expect(state.lists.map(list => list.record.name)).toEqual(["Home playlist", "xtream.example.invalid"]);
    expect(state.lists[0]).toMatchObject({ record: { id: listId, rev: 1, ...M3U }, keyRev: 1 });
    expect(state.lists[0].profiles).toEqual([living, kids]);
    expect(state.manifest.profiles.find(profile => profile.id === kids)?.lists).toEqual([listId]);
  });

  it("are only ciphertext on the server", () => {
    const { save, stored, living } = withTwoProfiles();
    save(state => withList(withList(state, M3U, [living]).state, XTREAM, [living]).state);
    const held = JSON.stringify(stored());
    for (const secret of [
      "example.invalid",
      "secret-token",
      "hunter2",
      "someone",
      "Living room",
      "Kids",
      "playlist",
    ])
      expect(held).not.toContain(secret);
  });

  it("keep a list's key when it's edited, and give it a new one when it's taken from a profile", () => {
    const { save, living, kids, stored } = withTwoProfiles();
    let listId = "";
    const first = save(state => {
      const added = withList(state, M3U, [living, kids]);
      listId = added.listId;
      return added.state;
    });
    const kidsCopy = stored().lists[0].profileKeys.find(entry => entry.profile === kids);
    const kidsKey = first.profiles.find(profile => profile.record.id === kids)?.key;
    if (!kidsCopy || !kidsKey) throw new Error("Kids has no copy of the list key");
    const keptByKidsTv = unwrapKey(kidsKey, kidsCopy.key);

    const edited = save(state => withList(state, { ...M3U, name: "Renamed" }, [living, kids], listId).state);
    expect(edited.lists[0]).toMatchObject({ record: { rev: 2, name: "Renamed" }, keyRev: 1 });

    const taken = save(state => withList(state, M3U, [living], listId).state);
    expect(taken.lists[0]).toMatchObject({ record: { rev: 3 }, keyRev: 2, profiles: [living] });
    const stillThere = stored().lists[0];
    expect(stillThere.profileKeys.map(entry => entry.profile)).toEqual([living]);
    // A TV of the Kids profile kept the old key: it opens no new version.
    expect(() => openJson(keptByKidsTv, stillThere.sealed)).toThrow(/Couldn't decrypt/);
  });

  it("give a list's new key a new revision when only its profiles change", () => {
    const { save, living, kids } = withTwoProfiles();
    let listId = "";
    save(state => {
      const added = withList(state, M3U, [living]);
      listId = added.listId;
      return added.state;
    });
    const given = save(state => withList(state, M3U, [living, kids], listId).state);
    expect(given.lists[0]).toMatchObject({ record: { rev: 2 }, keyRev: 1, profiles: [living, kids] });
  });

  it("re-key the lists of a deleted profile", () => {
    const { save, living, kids } = withTwoProfiles();
    save(state => withList(withList(state, M3U, [living, kids]).state, XTREAM, [living]).state);
    const after = save(state => withoutProfile(state, kids));
    expect(after.profiles.map(profile => profile.record.id)).toEqual([living]);
    expect(after.lists.map(list => [list.keyRev, list.profiles])).toEqual([
      [2, [living]],
      [1, [living]],
    ]);
  });

  it("delete lists", () => {
    const { save, living } = withTwoProfiles();
    let listId = "";
    save(state => {
      const added = withList(state, M3U, [living]);
      listId = added.listId;
      return added.state;
    });
    expect(save(state => withoutList(state, listId)).lists).toEqual([]);
  });

  it("stop at the limits per account", () => {
    const { save } = setup();
    expect(() =>
      save(state => {
        let next = state;
        for (let i = 0; i <= MAX_PROFILES; i++) next = withProfile(next, { name: `Profile ${i}` }).state;
        return next;
      }),
    ).toThrow(/at most 8 profiles/);
    expect(() =>
      save(state => {
        let next = state;
        for (let i = 0; i <= MAX_LISTS; i++) next = withList(next, M3U, []).state;
        return next;
      }),
    ).toThrow(/at most 50 lists/);
  });
});

const refusal = (run: () => unknown) => {
  try {
    run();
  } catch (error) {
    if (error instanceof ChangeProblem) return error.code;
    throw error;
  }
  return "accepted";
};

describe("the server", () => {
  it("answers 'behind' to a change built on an older revision", () => {
    const { owner, open, server, living } = withTwoProfiles();
    const before = open();
    server(buildChange(owner, before, withList(before, M3U, [living]).state));
    // A second tab, still on the old state.
    expect(refusal(() => server(buildChange(owner, before, withList(before, XTREAM, [living]).state)))).toBe(
      "behind",
    );
  });

  it("refuses a manifest not signed by the owner", () => {
    const { owner, open, server } = setup();
    const before = open();
    const change = buildChange(owner, before, withProfile(before, { name: "Living room" }).state);
    const forged = {
      ...change,
      manifest: signJson(newOwnerKeys().signing.secretKey, JSON.parse(atob(change.manifest.body))),
    };
    expect(refusal(() => server(forged))).toBe("refused");
  });

  it("refuses records the manifest doesn't name, or at another revision", () => {
    const { owner, open, server, living } = withTwoProfiles();
    const before = open();
    const change = buildChange(owner, before, withList(before, M3U, [living]).state);
    expect(refusal(() => server({ ...change, putLists: [] }))).toBe("refused");
    expect(
      refusal(() => server({ ...change, putLists: change.putLists.map(list => ({ ...list, rev: 7 })) })),
    ).toBe("refused");
    expect(
      refusal(() =>
        server({ ...change, putLists: change.putLists.map(list => ({ ...list, profileKeys: [] })) }),
      ),
    ).toBe("refused");
    expect(refusal(() => server(change))).toBe("accepted");
  });

  it("refuses to put back an older revision", () => {
    const { owner, open, server, stored, living } = withTwoProfiles();
    let listId = "";
    const first = open();
    const added = withList(first, M3U, [living]);
    listId = added.listId;
    server(buildChange(owner, first, added.state));
    const oldRecord = stored().lists[0];
    const second = open();
    server(buildChange(owner, second, withList(second, XTREAM, [living], listId).state));
    // A signed manifest that names the old revision, as a replayed change would.
    const third = open();
    const replay = buildChange(owner, third, third);
    const body: unknown = JSON.parse(atob(replay.manifest.body));
    if (!isRecord(body)) throw new Error("The manifest isn't an object");
    const manifest = { ...body, lists: [{ id: listId, rev: 1, keyRev: 1 }] };
    expect(
      refusal(() =>
        server({ ...replay, manifest: signJson(owner.signing.secretKey, manifest), putLists: [oldRecord] }),
      ),
    ).toBe("refused");
  });

  it("refuses past the limits even when the owner signed it", () => {
    const { owner, open, server } = setup();
    const before = open();
    let next = before;
    for (let i = 0; i < MAX_PROFILES; i++) next = withProfile(next, { name: `Profile ${i}` }).state;
    server(buildChange(owner, before, next));
    const full = open();
    // Built past the builder's own check.
    const extra = {
      ...full,
      profiles: [
        ...full.profiles,
        { ...full.profiles[0], record: { ...full.profiles[0].record, id: "extra" } },
      ],
    };
    expect(refusal(() => server(buildChange(owner, full, extra)))).not.toBe("accepted");
  });

  it("refuses changes that aren't readable", () => {
    const { server } = setup();
    expect(refusal(() => server({ format: 2 }))).toBe("refused");
    expect(refusal(() => server(null))).toBe("refused");
  });
});

describe("the owner's device", () => {
  it("refuses a server that serves an old version of a list", () => {
    const { owner, open, server, stored, living } = withTwoProfiles();
    const first = open();
    const added = withList(first, M3U, [living]);
    server(buildChange(owner, first, added.state));
    const old = stored().lists[0];
    const second = open();
    server(buildChange(owner, second, withList(second, XTREAM, [living], added.listId).state));
    const tampered: StoredState = { ...stored(), lists: [old] };
    expect(() => openState(owner, ACCOUNT, tampered)).toThrow(/revision/);
  });

  it("refuses a manifest from another key, or for another account", () => {
    const { owner, stored } = withTwoProfiles();
    expect(() => openState(newOwnerKeys(), ACCOUNT, stored())).toThrow(/signature/);
    expect(() => openState(owner, "another-account", stored())).toThrow(/another account/);
  });

  it("refuses a list key for a profile that isn't the list's key", () => {
    const { owner, open, server, stored, living, kids } = withTwoProfiles();
    const before = open();
    server(
      buildChange(
        owner,
        before,
        withList(withList(before, M3U, [living, kids]).state, XTREAM, [living, kids]).state,
      ),
    );
    const [a, b] = stored().lists;
    const swapped: StoredState = {
      ...stored(),
      lists: [{ ...a, profileKeys: b.profileKeys }, b],
    };
    expect(() => openState(owner, ACCOUNT, swapped)).toThrow(/isn't its key/);
  });
});

describe("list checks", () => {
  it("are VelioTV's sourceProblem(), plus sizes and refresh choices", () => {
    expect(listProblem(M3U)).toBeUndefined();
    expect(listProblem(XTREAM)).toBeUndefined();
    expect(listProblem({ ...M3U, url: "" })).toBe("url");
    expect(listProblem({ ...M3U, url: "ftp://x" })).toBe("urlForm");
    expect(listProblem({ ...XTREAM, server: "" })).toBe("server");
    expect(listProblem({ ...XTREAM, password: "" })).toBe("login");
    expect(listProblem({ ...M3U, guideUrls: ["guide.xml"] })).toBe("guideForm");
    expect(listProblem({ ...M3U, name: "x".repeat(101) })).toBe("tooLong");
    expect(listProblem({ ...M3U, refreshHours: 5 })).toBe("refresh");
  });

  it("name an unnamed list after its host", () => {
    expect(defaultListName(XTREAM)).toBe("xtream.example.invalid");
    expect(defaultListName({ ...XTREAM, server: "xtream.example.invalid:8080" })).toBe(
      "xtream.example.invalid",
    );
  });

  it("read lists loosely, keeping what fits", () => {
    expect(() => readList({ format: 1, id: "a", rev: 1, name: "x", kind: "rtsp", refreshHours: 24 })).toThrow(
      /kind/,
    );
    expect(
      readList({ format: 1, id: "a", rev: 1, name: "x", kind: "m3u", refreshHours: 24, disabled: false }),
    ).toEqual({ format: 1, id: "a", rev: 1, name: "x", kind: "m3u", refreshHours: 24 });
  });
});
