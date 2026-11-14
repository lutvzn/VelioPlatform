// Writes the vectors the Rust server replays (PLAN.md, "Moving off
// Cloudflare", "Checking it matches"): applyChange() and pullFor() cases,
// with this TypeScript as the reference. Each change case has the state
// before, the input, and the state after or the problem.
//
// test/vectors.test.ts compares apps/server/tests/vectors.json with what this
// builds, so `pnpm check` fails when they differ; `pnpm vectors` writes it.
//
// Random bytes come from a seeded generator, so the file is the same each run.

import nacl from "tweetnacl";
import { fromBase64, toBase64, utf8 } from "../src/bytes.ts";
import {
  applyChange,
  buildChange,
  ChangeProblem,
  openState,
  readStoredState,
  withDevice,
  withDeviceProfiles,
  withList,
  withoutDevice,
  withoutList,
  withoutProfile,
  withProfile,
  type Change,
  type OpenedState,
  type StoredState,
} from "../src/changes.ts";
import { isRecord, signJson } from "../src/crypto.ts";
import { pullFor } from "../src/device.ts";
import { newOwnerKeys, ownerPublicKeys } from "../src/owner.ts";
import type { ListDraft } from "../src/records.ts";

export const VECTORS_FILE = new URL("../../../apps/server/tests/vectors.json", import.meta.url);

// --- A seeded generator: SHA-512 of the seed and a counter ---

let counter = 0;
nacl.setPRNG((out: Uint8Array, length: number) => {
  for (let at = 0; at < length;) {
    const block = nacl.hash(utf8(`velio vectors ${counter++}`));
    const take = Math.min(block.length, length - at);
    out.set(block.subarray(0, take), at);
    at += take;
  }
});

const ACCOUNT = "account-vectors";
const owner = newOwnerKeys();
const stranger = newOwnerKeys();

/** Through JSON, as over the network and on the page. */
const json = <T>(value: T): T => JSON.parse(JSON.stringify(value));

const EMPTY: StoredState = { manifest: null, profiles: [], lists: [], devices: [] };

// --- The cases ---

type Expect =
  | { state: string }
  | { problem: { code: string; message: string } }
  | { problem: { code: string; messageStartsWith: string } };

interface ChangeCase {
  name: string;
  stored: string;
  input: unknown;
  newDevices?: string[];
  deviceLimit?: number;
  expect: Expect;
}

const states: Record<string, StoredState> = { empty: EMPTY };
const changes: ChangeCase[] = [];
const pulls: { name: string; stored: string; device: string; expect: unknown }[] = [];

/** Records one case; answers the state after, or undefined. */
const run = (
  name: string,
  before: string,
  input: unknown,
  newDevices?: string[],
  after?: string,
  deviceLimit?: number,
) => {
  const sent = json(input);
  let expect: Expect;
  let next: StoredState | undefined;
  try {
    next = json(
      applyChange(states[before], sent, ACCOUNT, owner.signing.publicKey, { newDevices, deviceLimit }).state,
    );
    const known = Object.entries(states).find(([, state]) => JSON.stringify(state) === JSON.stringify(next));
    const stateName = known?.[0] ?? after ?? `after ${name}`;
    if (!known && stateName in states)
      throw new Error(`${name}: another state is already called ${stateName}`);
    states[stateName] = next;
    expect = { state: stateName };
  } catch (error) {
    if (!(error instanceof ChangeProblem)) throw error;
    // V8's words for bad JSON aren't serde's: only the part before them is compared.
    const at = error.message.indexOf("SyntaxError");
    expect =
      at === -1
        ? { problem: { code: error.code, message: error.message } }
        : {
            problem: {
              code: error.code,
              messageStartsWith: error.message.slice(0, at + "SyntaxError".length),
            },
          };
  }
  changes.push({
    name,
    stored: before,
    input: sent,
    ...(newDevices ? { newDevices } : {}),
    ...(deviceLimit === undefined ? {} : { deviceLimit }),
    expect,
  });
  return next;
};

const accepted = (name: string, before: string, input: unknown, after: string, newDevices?: string[]) => {
  if (!run(name, before, input, newDevices, after)) throw new Error(`${name} was refused`);
  return after;
};

const refusedCase = (name: string, before: string, input: unknown, newDevices?: string[]) => {
  if (run(name, before, input, newDevices)) throw new Error(`${name} was accepted`);
};

const open = (name: string) => openState(owner, ACCOUNT, readStoredState(json(states[name])));
const change = (name: string, edit: (state: OpenedState) => OpenedState): Change => {
  const before = open(name);
  return json(buildChange(owner, before, edit(before)));
};

const pull = (name: string, stored: string, device: string) =>
  pulls.push({ name, stored, device, expect: json(pullFor(states[stored], device)) });

// --- Accepted changes of every kind, one after another ---

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
const ADDON: ListDraft = {
  name: "Films",
  kind: "stremio",
  url: "stremio://addon.example.invalid",
  refreshHours: 24,
};

let living = "";
let kids = "";
let m3u = "";
let xtream = "";

accepted(
  "adds the first profile",
  "empty",
  change("empty", state => {
    const made = withProfile(state, { name: "Living room" });
    living = made.profileId;
    return made.state;
  }),
  "one profile",
);
accepted(
  "adds a profile with a colour and a PIN",
  "one profile",
  change("one profile", state => {
    const made = withProfile(state, { name: "Kids", color: 3, pin: "1234" });
    kids = made.profileId;
    return made.state;
  }),
  "two profiles",
);
accepted(
  "adds an M3U list for both profiles",
  "two profiles",
  change("two profiles", state => {
    const made = withList(state, M3U, [living, kids]);
    m3u = made.listId;
    return made.state;
  }),
  "one list",
);
accepted(
  "adds an Xtream list for no profile and an add-on for Kids",
  "one list",
  change("one list", state => {
    const first = withList(state, XTREAM, []);
    const second = withList(first.state, ADDON, [kids]);
    xtream = first.listId;
    return second.state;
  }),
  "three lists",
);
accepted(
  "renames a profile",
  "three lists",
  change("three lists", state => withProfile(state, { name: "The living room" }, living).state),
  "renamed",
);
accepted(
  "takes a list from a profile, with a new key",
  "renamed",
  change("renamed", state => withList(state, M3U, [living], m3u).state),
  "list taken",
);
accepted(
  "gives the Xtream list to a profile and turns it off",
  "list taken",
  change("list taken", state => withList(state, { ...XTREAM, disabled: true }, [kids], xtream).state),
  "list given",
);

const tvKeys = nacl.box.keyPair();
const secondTvKeys = nacl.box.keyPair();
accepted(
  "signs in a TV",
  "list given",
  change("list given", state => withDevice(state, "tv-1", tvKeys.publicKey, [living], "Living room TV")),
  "one TV",
  ["tv-1"],
);
accepted(
  "signs in a second TV with both profiles",
  "one TV",
  change("one TV", state => withDevice(state, "tv-2", secondTvKeys.publicKey, [living, kids], "Kids TV")),
  "two TVs",
  ["tv-2"],
);
accepted(
  "gives a TV another profile",
  "two TVs",
  change("two TVs", state => withDeviceProfiles(state, "tv-1", [living, kids])),
  "TV given",
);
accepted(
  "takes a profile from a TV, with new keys",
  "TV given",
  change("TV given", state => withDeviceProfiles(state, "tv-1", [living])),
  "TV taken",
);
accepted(
  "deletes a list",
  "TV taken",
  change("TV taken", state => withoutList(state, xtream)),
  "list deleted",
);
accepted(
  "deletes a profile a TV has",
  "list deleted",
  change("list deleted", state => withoutProfile(state, kids)),
  "profile deleted",
);
accepted(
  "removes a TV",
  "profile deleted",
  change("profile deleted", state => withoutDevice(state, "tv-2")),
  "TV removed",
);

for (const stored of ["empty", "one TV", "two TVs", "TV taken", "profile deleted", "TV removed"])
  for (const device of ["tv-1", "tv-2", "tv-3"]) pull(`${device} pulls at "${stored}"`, stored, device);

// --- Changes made by hand, from one base ---

// Edits put what the types don't allow, so they work on plain JSON by path.
type Path = readonly (string | number)[];
const REMOVE = Symbol("remove");

/** A copy of a JSON value with what's at the path replaced, or removed. */
const setAt = (value: unknown, path: Path, next: unknown): unknown => {
  const [key, ...rest] = path;
  if (key === undefined) return next;
  if (Array.isArray(value) && typeof key === "number") {
    const copy: unknown[] = [...value];
    copy[key] = setAt(copy[key], rest, next);
    return copy;
  }
  if (isRecord(value) && typeof key === "string") {
    const copy = { ...value };
    if (rest.length === 0 && next === REMOVE) delete copy[key];
    else copy[key] = setAt(copy[key], rest, next);
    return copy;
  }
  throw new Error(`Nothing at ${String(key)}`);
};
const getAt = (value: unknown, path: Path): unknown =>
  path.reduce<unknown>((at, key) => {
    if (Array.isArray(at) && typeof key === "number") return at[key];
    if (isRecord(at) && typeof key === "string") return at[key];
    throw new Error(`Nothing at ${String(key)}`);
  }, value);
const listAt = (value: unknown, path: Path): unknown[] => {
  const found = getAt(value, path);
  if (!Array.isArray(found)) throw new Error(`No list at ${path.join(".")}`);
  return found;
};
const numberAt = (value: unknown, path: Path): number => {
  const found = getAt(value, path);
  if (typeof found !== "number") throw new Error(`No number at ${path.join(".")}`);
  return found;
};
/** The value with each [path, value] edit made. */
const withEdits = (value: unknown, ...edits: [Path, unknown][]) =>
  edits.reduce((at, [path, next]) => setAt(at, path, next), value);

const BASE = "two TVs";
/** A valid change from the base: a profile renamed and a list added. */
const valid = (): Change =>
  change(
    BASE,
    state => withList(withProfile(state, { name: "Kids, renamed" }, kids).state, M3U, [kids]).state,
  );
const edited = (...edits: [Path, unknown][]) => withEdits(valid(), ...edits);
/** A typed copy of the valid change, edited in place. */
const tweak = (input: Change, edit: (input: Change) => void) => {
  edit(input);
  return input;
};
/** The valid change with its manifest edited and signed again. */
const resigned = (edit: (manifest: Record<string, unknown>) => unknown, by = owner, from = valid()) => {
  const manifest: unknown = JSON.parse(new TextDecoder().decode(fromBase64(from.manifest.body)));
  if (!isRecord(manifest)) throw new Error("No manifest");
  return { ...from, manifest: signJson(by.signing.secretKey, edit(manifest)) };
};
const signedBytes = (bytes: Uint8Array) => ({
  ...valid(),
  manifest: {
    body: toBase64(bytes),
    signature: toBase64(nacl.sign.detached(bytes, owner.signing.secretKey)),
  },
});
const longText = (length: number) => "A".repeat(length);
const NONCE: Path = ["putProfiles", 0, "sealed", "nonce"];

accepted("a valid change", BASE, valid(), "valid");
accepted(
  "a manifest's body without base64 padding",
  BASE,
  tweak(valid(), input => (input.manifest.body = input.manifest.body.replace(/=+$/, ""))),
  "valid, unpadded",
);
accepted(
  "a nonce of 16 emoji, which is 32 in JavaScript",
  BASE,
  edited([NONCE, "😀".repeat(16)]),
  "emoji nonce",
);
refusedCase("a nonce of 17 emoji", BASE, edited([NONCE, "😀".repeat(17)]));
accepted(
  "no TVs, as pages from before Phase 3 send",
  "list given",
  withEdits(
    change("list given", state => state),
    [["putDevices"], REMOVE],
    [["deleteDevices"], REMOVE],
  ),
  "nothing changed",
);

// readChange()
refusedCase("a change that isn't an object", BASE, []);
refusedCase("a change that's null", BASE, null);
refusedCase("a change that's text", BASE, "change");
const firstKey = valid().putLists[0].profileKeys[0];
const readCases: [string, Path, unknown][] = [
  ["an unknown format", ["format"], 2],
  ["no format", ["format"], REMOVE],
  ["a baseRev as text", ["baseRev"], "2"],
  ["a negative baseRev", ["baseRev"], -1],
  ["a baseRev that isn't whole", ["baseRev"], 1.5],
  ["a baseRev past safe integers", ["baseRev"], 2 ** 53],
  ["no manifest", ["manifest"], REMOVE],
  ["a manifest's body that's a number", ["manifest", "body"], 1],
  ["a manifest's body that's too long", ["manifest", "body"], longText(64_001)],
  ["a manifest's signature that's too long", ["manifest", "signature"], longText(89)],
  ["no changed profiles", ["putProfiles"], REMOVE],
  ["too many changed profiles", ["putProfiles"], Array(9).fill(0)],
  ["a changed profile that isn't an object", ["putProfiles"], [5]],
  ["a profile id with a space", ["putProfiles", 0, "id"], "a b"],
  ["a profile id that's too long", ["putProfiles", 0, "id"], longText(41)],
  ["an empty profile id", ["putProfiles", 0, "id"], ""],
  ["a negative profile rev", ["putProfiles", 0, "rev"], -1],
  ["a profile without its sealed record", ["putProfiles", 0, "sealed"], REMOVE],
  ["a nonce that's too short", NONCE, longText(31)],
  ["a nonce that's too long", NONCE, longText(33)],
  ["a nonce that's a number", NONCE, 1],
  ["a profile box that's too big", ["putProfiles", 0, "sealed", "box"], longText(4097)],
  ["a wrapped key that's too short", ["putProfiles", 0, "key", "box"], longText(71)],
  ["a wrapped key that's too long", ["putProfiles", 0, "key", "box"], longText(73)],
  ["a deleted profile that isn't an id", ["deleteProfiles"], [5]],
  ["too many deleted profiles", ["deleteProfiles"], Array(9).fill("p")],
  ["too many changed lists", ["putLists"], Array(51).fill(0)],
  ["a list without keyRev", ["putLists", 0, "keyRev"], REMOVE],
  ["a list box that's too big", ["putLists", 0, "sealed", "box"], longText(48_001)],
  ["a list's profile keys that aren't a list", ["putLists", 0, "profileKeys"], {}],
  [
    "too many profile keys on a list",
    ["putLists", 0, "profileKeys"],
    Array.from({ length: 9 }, () => json(firstKey)),
  ],
  ["a list's profile key of the wrong size", ["putLists", 0, "profileKeys", 0, "key", "box"], longText(70)],
  ["a list's profile key for a bad id", ["putLists", 0, "profileKeys", 0, "profile"], "no/slash"],
  ["deleted lists that aren't a list", ["deleteLists"], "all"],
  ["changed TVs that are null", ["putDevices"], null],
  ["too many changed TVs", ["putDevices"], Array(11).fill(0)],
  ["a TV with a bad id", ["putDevices"], [{ id: "tv 1", keys: [] }]],
  ["a TV whose keys aren't a list", ["putDevices"], [{ id: "tv-1", keys: null }]],
  ["removed TVs as text", ["deleteDevices"], "tv-1"],
];
for (const [name, path, value] of readCases) refusedCase(name, BASE, edited([path, value]));

// Behind
refusedCase(
  "an older revision",
  BASE,
  tweak(valid(), input => (input.baseRev -= 1)),
);
refusedCase(
  "a newer revision",
  BASE,
  tweak(valid(), input => (input.baseRev += 1)),
);
refusedCase(
  "a change from the empty account, sent twice",
  "one profile",
  change("empty", state => state),
);

// The signature and the manifest
refusedCase(
  "a manifest signed by someone else",
  BASE,
  resigned(manifest => manifest, stranger),
);
refusedCase("a signature that isn't base64", BASE, edited([["manifest", "signature"], "!!!!"]));
refusedCase(
  "a signature of 63 bytes",
  BASE,
  edited([["manifest", "signature"], toBase64(new Uint8Array(63))]),
);
refusedCase(
  "a manifest changed after signing",
  BASE,
  edited([["manifest", "body"], resigned(manifest => ({ ...manifest, rev: 99 })).manifest.body]),
);
refusedCase("a manifest's body that isn't base64", BASE, edited([["manifest", "body"], "@@@@"]));
refusedCase("a manifest's body of the wrong base64 length", BASE, edited([["manifest", "body"], "QUFBQ"]));
refusedCase("a signed body that isn't UTF-8", BASE, signedBytes(new Uint8Array([0x7b, 0xff, 0x7d])));
refusedCase("a signed body cut short in a character", BASE, signedBytes(new Uint8Array([0x22, 0xe2, 0x82])));
refusedCase("a signed body that isn't JSON", BASE, signedBytes(utf8("not json")));
/** A manifest's TVs, with one changed. */
const deviceEdited = (manifest: Record<string, unknown>, deviceId: string, profiles: string[]) =>
  listAt(manifest, ["devices"]).map(device =>
    isRecord(device) && device.id === deviceId ? { ...device, profiles } : device,
  );
const manifestCases: [string, (manifest: Record<string, unknown>) => unknown][] = [
  ["a signed manifest that isn't an object", () => [1]],
  ["a signed manifest of an unknown format", manifest => ({ ...manifest, format: 2 })],
  ["a manifest's account that's too long", manifest => ({ ...manifest, account: longText(65) })],
  ["a manifest's rev that's text", manifest => ({ ...manifest, rev: "3" })],
  [
    "a manifest with a profile twice",
    manifest => {
      const profiles = listAt(manifest, ["profiles"]);
      return { ...manifest, profiles: [...profiles, profiles[0]] };
    },
  ],
  ["a manifest whose profile has a list it doesn't have", manifest => ({ ...manifest, lists: [] })],
  [
    "a manifest whose profile has a list twice",
    manifest => ({
      ...manifest,
      profiles: listAt(manifest, ["profiles"]).map(profile => {
        const lists = listAt(profile, ["lists"]);
        return setAt(profile, ["lists"], [...lists, ...lists]);
      }),
    }),
  ],
  [
    "a manifest with a list twice",
    manifest => {
      const lists = listAt(manifest, ["lists"]);
      return { ...manifest, lists: [...lists, lists[0]] };
    },
  ],
  [
    "a manifest with 11 TVs",
    manifest => ({ ...manifest, devices: Array.from({ length: 11 }, () => ({ id: "tv", profiles: [] })) }),
  ],
  [
    "a manifest TV with a public key that isn't base64",
    manifest => ({ ...manifest, devices: [{ id: "tv-1", profiles: [], publicKey: `${longText(42)}==` }] }),
  ],
  [
    "a manifest TV with a public key that's too long",
    manifest => ({ ...manifest, devices: [{ id: "tv-1", profiles: [], publicKey: longText(45) }] }),
  ],
  [
    "a manifest TV whose name isn't sealed",
    manifest => ({ ...manifest, devices: [{ id: "tv-1", profiles: [], name: "Living room TV" }] }),
  ],
  [
    "a manifest TV whose sealed name is too long",
    manifest => ({
      ...manifest,
      devices: [{ id: "tv-1", profiles: [], name: { nonce: longText(32), box: longText(1025) } }],
    }),
  ],
  [
    "a manifest TV whose sealed name has a short nonce",
    manifest => ({
      ...manifest,
      devices: [{ id: "tv-1", profiles: [], name: { nonce: longText(31), box: longText(364) } }],
    }),
  ],
  [
    "a manifest list's keyRev that's negative",
    manifest => ({ ...manifest, lists: [{ id: "l", rev: 1, keyRev: -1 }] }),
  ],
  ["a manifest for another account", manifest => ({ ...manifest, account: "someone-else" })],
  ["a manifest that skips a revision", manifest => ({ ...manifest, rev: numberAt(manifest, ["rev"]) + 1 })],
  ["a manifest at the same revision", manifest => ({ ...manifest, rev: numberAt(manifest, ["rev"]) - 1 })],
  // checkMatches(): the TV is told it has a profile the account doesn't.
  [
    "a TV given a profile the account doesn't have",
    manifest => ({ ...manifest, devices: deviceEdited(manifest, "tv-1", ["ghost"]) }),
  ],
  // sameSet() takes a manifest that names a TV's profile twice; Rust has to as well.
  [
    "a TV whose profile the manifest names twice",
    manifest => ({ ...manifest, devices: deviceEdited(manifest, "tv-2", [living, living]) }),
  ],
];
for (const [name, edit] of manifestCases) run(name, BASE, resigned(edit));

// applyChange()'s own checks
refusedCase(
  "a change that names a profile twice",
  BASE,
  tweak(valid(), input => input.deleteProfiles.push(kids)),
);
refusedCase(
  "deleting a profile that doesn't exist",
  BASE,
  tweak(valid(), input => input.deleteProfiles.push("nope")),
);
refusedCase(
  "deleting a list that doesn't exist",
  BASE,
  tweak(valid(), input => input.deleteLists.push("nope")),
);
refusedCase(
  "removing a TV that doesn't exist",
  BASE,
  tweak(valid(), input => input.deleteDevices.push("tv-9")),
);
refusedCase(
  "a profile whose revision doesn't go up",
  BASE,
  tweak(valid(), input => (input.putProfiles[0].rev -= 1)),
);
const listEdit = () => change(BASE, state => withList(state, { ...M3U, name: "Again" }, [living], m3u).state);
refusedCase(
  "a list whose revision doesn't go up",
  BASE,
  tweak(listEdit(), input => (input.putLists[0].rev -= 1)),
);
refusedCase(
  "a list whose key revision goes down",
  BASE,
  tweak(listEdit(), input => (input.putLists[0].keyRev -= 1)),
);
const signIn = () => change(BASE, state => withDevice(state, "tv-3", tvKeys.publicKey, [kids], "Kitchen TV"));
refusedCase("a TV added without signing it in", BASE, signIn());
refusedCase("a TV added when another is signing in", BASE, signIn(), ["tv-4"]);
accepted("a TV added while it signs in", BASE, signIn(), "three TVs", ["tv-3"]);

// The account's TV limit: BASE has two TVs.
if (run("a TV past the account's limit", BASE, signIn(), ["tv-3"], undefined, 2))
  throw new Error("a TV past the account's limit was accepted");
if (!run("a TV within a limit the operator gave", BASE, signIn(), ["tv-3"], undefined, 3))
  throw new Error("a TV within a limit the operator gave was refused");
if (!run("a change to an account with more TVs than its limit", BASE, listEdit(), undefined, undefined, 1))
  throw new Error("a change to an account with more TVs than its limit was refused");

// checkRekeyed(): TV 1 has both profiles in "TV given".
const GIVEN = "TV given";
refusedCase(
  "a TV that loses a profile without a new key",
  GIVEN,
  change(GIVEN, state => ({
    ...state,
    devices: state.devices.map(device => (device.id === "tv-1" ? { ...device, profiles: [living] } : device)),
  })),
);
refusedCase(
  "a new profile key not sent to a TV that keeps it",
  GIVEN,
  tweak(
    change(GIVEN, state => withDeviceProfiles(state, "tv-1", [living])),
    input => (input.putDevices = input.putDevices.filter(device => device.id !== "tv-2")),
  ),
);
refusedCase(
  "a list a TV can't use any more, without a new key",
  GIVEN,
  change(GIVEN, state => {
    const rekeyed = withDeviceProfiles(state, "tv-1", [living]);
    // The profile got its new key, but its lists kept theirs.
    return { ...rekeyed, lists: state.lists };
  }),
);

// checkMatches()
const addProfile = () => change(BASE, state => withProfile(state, { name: "Guest" }).state);
const addList = () => change(BASE, state => withList(state, XTREAM, [living]).state);
refusedCase(
  "a profile the manifest names, not sent",
  BASE,
  tweak(addProfile(), input => (input.putProfiles = [])),
);
refusedCase(
  "a profile at another revision than the manifest's",
  BASE,
  tweak(addProfile(), input => (input.putProfiles[0].rev = 2)),
);
refusedCase(
  "a list the manifest names, not sent",
  BASE,
  tweak(addList(), input => (input.putLists = [])),
);
refusedCase(
  "a list at another key revision than the manifest's",
  BASE,
  tweak(addList(), input => (input.putLists[0].keyRev = 2)),
);
refusedCase(
  "a list without the keys of the profiles that have it",
  BASE,
  tweak(addList(), input => (input.putLists[0].profileKeys = [])),
);
refusedCase(
  "a list with one profile's key twice",
  BASE,
  tweak(
    change(BASE, state => withList(state, XTREAM, [living, kids]).state),
    input => (input.putLists[0].profileKeys[1] = input.putLists[0].profileKeys[0]),
  ),
);
refusedCase(
  "a TV the manifest names, not sent",
  BASE,
  tweak(signIn(), input => (input.putDevices = [])),
  ["tv-3"],
);
refusedCase(
  "a TV sent with keys for other profiles",
  BASE,
  tweak(signIn(), input => (input.putDevices[0].keys[0].profile = living)),
  ["tv-3"],
);

// --- Writing ---

const used = new Set([...changes.flatMap(item => [item.stored]), ...pulls.map(item => item.stored)]);
for (const item of changes) if ("state" in item.expect) used.add(item.expect.state);
for (const name of Object.keys(states)) if (!used.has(name)) delete states[name];

/** The file, as the Rust tests read it. */
export const vectorsText = `${JSON.stringify({
  account: ACCOUNT,
  signPublicKey: ownerPublicKeys(owner).signPublicKey,
  states,
  changes,
  pulls,
})}\n`;
export const vectorCounts = { changes: changes.length, pulls: pulls.length };
