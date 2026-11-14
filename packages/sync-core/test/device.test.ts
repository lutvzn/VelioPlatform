// Signing in a TV and its pulls, between an owner device, a server and a TV
// that only talk through what they send each other (PLAN.md, Phase 3).

import nacl from "tweetnacl";
import { describe, expect, it } from "vitest";
import { fromBase64, toBase64 } from "../src/bytes.ts";
import {
  applyChange,
  buildChange,
  openState,
  readStoredState,
  storedManifest,
  withDevice,
  withDeviceProfiles,
  withList,
  withoutDevice,
  withoutProfile,
  withProfile,
  type ChangeOptions,
  type OpenedState,
  type StoredState,
} from "../src/changes.ts";
import { openBoxedKey, openJson, signJson, unwrapKey, type Sealed } from "../src/crypto.ts";
import { DeviceRemoved, openPull, pullFor, type DeviceTrust, type DeviceView } from "../src/device.ts";
import { newOwnerKeys, ownerPublicKeys } from "../src/owner.ts";
import {
  newPairingId,
  newPairingSecrets,
  openDeviceInfo,
  openPairingPayload,
  pairingCode,
  pairingUrl,
  readPairingCode,
  sealDeviceInfo,
  sealPairingPayload,
} from "../src/pairing.ts";
import {
  DEFAULT_DEVICE_LIMIT,
  MAX_DEVICE_NAME_LENGTH,
  MAX_DEVICES,
  MAX_LISTS,
  MAX_PROFILES,
  newId,
  type ListDraft,
} from "../src/records.ts";

const ACCOUNT = "account-1";
const SITE = "https://velio.tv";

const list = (name: string): ListDraft => ({
  name,
  kind: "m3u",
  url: `http://${name.toLowerCase().replace(/\s/g, "-")}.example.invalid/list.m3u`,
  refreshHours: 24,
});

const roundTrip = (value: unknown): unknown => JSON.parse(JSON.stringify(value));

/** An account with three profiles, each with a list of its own. */
const setup = () => {
  const owner = newOwnerKeys();
  let stored: StoredState = { manifest: null, profiles: [], lists: [], devices: [] };
  const server = (change: unknown, options?: ChangeOptions) => {
    stored = applyChange(stored, roundTrip(change), ACCOUNT, owner.signing.publicKey, options).state;
  };
  const open = () => openState(owner, ACCOUNT, readStoredState(roundTrip(stored)));
  const save = (edit: (state: OpenedState) => OpenedState, options?: ChangeOptions) => {
    const before = open();
    server(buildChange(owner, before, edit(before)), options);
  };
  const profiles: string[] = [];
  save(state => {
    let next = state;
    for (const name of ["Living room", "Kids", "Guest"]) {
      const made = withProfile(next, { name });
      profiles.push(made.profileId);
      next = withList(made.state, list(`${name} list`), [made.profileId]).state;
    }
    return next;
  });
  return { owner, server, open, save, profiles, stored: () => stored };
};

/** The TV's side of signing in: its keys, secrets and what it posts. */
const newTv = (name = "Living room TV") => {
  const keys = nacl.box.keyPair();
  const secrets = newPairingSecrets();
  const publicKey = toBase64(keys.publicKey);
  return {
    keys,
    secrets,
    publicKey,
    pairingId: newPairingId(),
    sealed: sealDeviceInfo(secrets, { format: 1, publicKey, name }),
  };
};
type Tv = ReturnType<typeof newTv>;

/** The phone's side: reads the code, opens the TV's details, gives it profiles, and answers the TV. */
const signIn = (
  account: ReturnType<typeof setup>,
  tv: Tv,
  profiles: string[],
  typed = false,
  deviceLimit?: number,
) => {
  const code = readPairingCode(
    typed ? pairingCode(tv.pairingId, tv.secrets) : pairingUrl(SITE, tv.pairingId, tv.secrets),
  );
  if (!code) throw new Error("The code didn't read");
  const info = openDeviceInfo(code, tv.sealed);
  const deviceId = newId();
  account.save(
    state => withDevice(state, deviceId, fromBase64(info.publicKey), profiles, info.name, deviceLimit),
    {
      newDevices: [deviceId],
      deviceLimit,
    },
  );
  const publicKeys = ownerPublicKeys(account.owner);
  const sealed = sealPairingPayload(code, {
    format: 1,
    account: ACCOUNT,
    device: deviceId,
    devicePublicKey: info.publicKey,
    ...publicKeys,
  });
  // The TV opens what the phone sent.
  const payload = openPairingPayload(code.kind, tv.secrets, sealed, tv.publicKey);
  const trust: DeviceTrust = {
    account: payload.account,
    device: payload.device,
    secretKey: toBase64(tv.keys.secretKey),
    signPublicKey: payload.signPublicKey,
    boxPublicKey: payload.boxPublicKey,
  };
  return { info, deviceId, trust };
};

const pull = (account: ReturnType<typeof setup>, deviceId: string) =>
  roundTrip(pullFor(account.stored(), deviceId));

describe("pairing codes", () => {
  it("read back from the QR address, its fragment, and the typed code", () => {
    const secrets = newPairingSecrets();
    const url = pairingUrl(SITE, "K7QM", secrets);
    expect(readPairingCode(url)).toEqual({ id: "K7QM", kind: "qr", secret: secrets.qr });
    expect(readPairingCode(url.slice(url.indexOf("#")))).toEqual({
      id: "K7QM",
      kind: "qr",
      secret: secrets.qr,
    });
    const typed = pairingCode("K7QM", secrets);
    expect(typed).toMatch(/^K7QM-[0-9A-Z]{4}-[0-9A-Z]{4}$/);
    expect(readPairingCode(typed)).toEqual({ id: "K7QM", kind: "code", secret: secrets.code });
  });

  it("forgive case, spaces and look-alike letters", () => {
    expect(readPairingCode(" k7qm 2xhf-9wrt ")).toEqual({ id: "K7QM", kind: "code", secret: "2XHF9WRT" });
    expect(readPairingCode("K7QM-2XHF-9WR0")).toEqual(readPairingCode("k7qm-2xhf-9wro"));
    expect(readPairingCode("K7QM-2XHF-9WRT-EXTRA")).toBeNull();
    expect(readPairingCode("not a code")).toBeNull();
  });

  it("make ids from the unambiguous alphabet", () => {
    for (let i = 0; i < 50; i++) expect(newPairingId()).toMatch(/^[0-9A-HJKMNP-TV-Z]{4}$/);
  });
});

describe("signing in a TV", () => {
  it("gives it only the profiles chosen, and their lists", () => {
    const account = setup();
    const tv = newTv();
    const [living, kids] = account.profiles;
    const { info, deviceId, trust } = signIn(account, tv, [living, kids]);
    expect(info.name).toBe("Living room TV");

    const view = openPull(trust, pull(account, deviceId));
    expect(view.profiles.map(profile => profile.name)).toEqual(["Living room", "Kids"]);
    expect(view.lists.map(item => item.name)).toEqual(["Living room list", "Kids list"]);
    // The Guest profile's list and key never reach the TV.
    const sent = JSON.stringify(pull(account, deviceId));
    expect(sent).not.toContain(account.profiles[2]);
    expect(account.open().devices).toEqual([
      { id: deviceId, profiles: [living, kids], publicKey: tv.keys.publicKey, name: "Living room TV" },
    ]);
  });

  it("keeps the TV's name only sealed under the account key, out of what the server and the TV read", () => {
    const account = setup();
    const { deviceId } = signIn(account, newTv(), [account.profiles[0]]);
    const stored = account.stored();
    expect(JSON.stringify(stored)).not.toContain("Living room TV");
    const [entry] = storedManifest(stored, ACCOUNT).devices;
    expect(entry.id).toBe(deviceId);
    expect(entry.name).toMatchObject({ nonce: expect.any(String), box: expect.any(String) });
    expect(JSON.stringify(pull(account, deviceId))).not.toContain("Living room TV");
    // Sealed again on each change, so the server can't tell when it changed.
    account.save(state => withProfile(state, { name: "Another" }).state);
    const [after] = storedManifest(account.stored(), ACCOUNT).devices;
    expect(after.name?.box).not.toBe(entry.name?.box);
    expect(account.open().devices[0].name).toBe("Living room TV");
  });

  it("signs in a TV that sent no name, and refuses one too long", () => {
    const account = setup();
    const { deviceId } = signIn(account, newTv("  "), [account.profiles[0]]);
    expect(account.open().devices.find(device => device.id === deviceId)?.name).toBeUndefined();
    expect(() =>
      withDevice(account.open(), "d", newTv().keys.publicKey, [account.profiles[0]], "x".repeat(41)),
    ).toThrow(/too long/);
  });

  it("works with the typed code too", () => {
    const account = setup();
    const tv = newTv();
    const { deviceId, trust } = signIn(account, tv, [account.profiles[1]], true);
    expect(openPull(trust, pull(account, deviceId)).profiles.map(profile => profile.name)).toEqual(["Kids"]);
  });

  it("fails when the code doesn't match the TV's secret", () => {
    const tv = newTv();
    const other = newPairingSecrets();
    const code = readPairingCode(pairingUrl(SITE, tv.pairingId, other));
    if (!code) throw new Error("The code didn't read");
    expect(() => openDeviceInfo(code, tv.sealed)).toThrow(/Couldn't decrypt/);
  });

  it("refuses an answer for another TV's key", () => {
    const tv = newTv();
    const code = readPairingCode(pairingUrl(SITE, tv.pairingId, tv.secrets));
    if (!code) throw new Error("The code didn't read");
    const owner = ownerPublicKeys(newOwnerKeys());
    const sealed = sealPairingPayload(code, {
      format: 1,
      account: ACCOUNT,
      device: "d1",
      devicePublicKey: toBase64(nacl.box.keyPair().publicKey),
      ...owner,
    });
    expect(() => openPairingPayload("qr", tv.secrets, sealed, tv.publicKey)).toThrow(/another TV/);
  });

  it("can't be done by the server alone: a TV the pairing didn't name is refused", () => {
    const account = setup();
    const tv = newTv();
    expect(() =>
      account.save(state => withDevice(state, "made-up", tv.keys.publicKey, [account.profiles[0]], "TV")),
    ).toThrow(/signing it in/);
  });

  it("stops at the account's limit of TVs", () => {
    const account = setup();
    for (let i = 0; i < DEFAULT_DEVICE_LIMIT; i++) signIn(account, newTv(`TV ${i}`), [account.profiles[0]]);
    const state = account.open();
    expect(() => withDevice(state, "one-more", newTv().keys.publicKey, [account.profiles[0]], "TV")).toThrow(
      `at most ${DEFAULT_DEVICE_LIMIT} TVs`,
    );
  });

  it("takes more TVs when the operator gave the account more, up to MAX_DEVICES", () => {
    const account = setup();
    const given = DEFAULT_DEVICE_LIMIT + 2;
    let last: ReturnType<typeof signIn> | undefined;
    for (let i = 0; i < given; i++)
      last = signIn(account, newTv(`TV ${i}`), [account.profiles[0]], false, given);
    expect(account.open().devices).toHaveLength(given);
    // And the TV takes a manifest naming more than the default.
    if (!last) throw new Error("No TV signed in");
    expect(openPull(last.trust, pull(account, last.deviceId)).rev).toBeGreaterThan(0);
    expect(() =>
      withDevice(account.open(), "one-more", newTv().keys.publicKey, [account.profiles[0]], "TV", given),
    ).toThrow(`at most ${given} TVs`);
    expect(() =>
      withDevice(account.open(), "one-more", newTv().keys.publicKey, [account.profiles[0]], "TV", 1000),
    ).not.toThrow();
  });

  it("fits MAX_DEVICES TVs with every profile in the manifest, at the most profiles and lists", () => {
    const owner = newOwnerKeys();
    let state = openState(
      owner,
      ACCOUNT,
      readStoredState({ manifest: null, profiles: [], lists: [], devices: [] }),
    );
    const profiles: string[] = [];
    for (let p = 0; p < MAX_PROFILES; p++) {
      const made = withProfile(state, { name: `Profile ${p}` });
      profiles.push(made.profileId);
      state = made.state;
    }
    for (let l = 0; l < MAX_LISTS; l++) state = withList(state, list(`List ${l}`), profiles).state;
    for (let d = 0; d < MAX_DEVICES; d++)
      state = withDevice(
        state,
        newId(),
        newTv().keys.publicKey,
        profiles,
        "x".repeat(MAX_DEVICE_NAME_LENGTH),
        MAX_DEVICES,
      );
    const before = openState(
      owner,
      ACCOUNT,
      readStoredState({ manifest: null, profiles: [], lists: [], devices: [] }),
    );
    const change = roundTrip(buildChange(owner, before, state));
    const newDevices = state.devices.map(device => device.id);
    const applied = applyChange(
      { manifest: null, profiles: [], lists: [], devices: [] },
      change,
      ACCOUNT,
      owner.signing.publicKey,
      { newDevices, deviceLimit: MAX_DEVICES },
    );
    expect(applied.manifest.devices).toHaveLength(MAX_DEVICES);
  });

  it("refuses, on the server, a TV past the account's limit", () => {
    const account = setup();
    signIn(account, newTv("One"), [account.profiles[0]]);
    expect(() => signIn(account, newTv("Two"), [account.profiles[0]], false, 1)).toThrow(/at most 1 TV\b/);
    // A browser told a higher limit than the server's still gets refused.
    const deviceId = newId();
    expect(() =>
      account.save(
        state => withDevice(state, deviceId, newTv().keys.publicKey, [account.profiles[0]], "TV", 5),
        { newDevices: [deviceId], deviceLimit: 1 },
      ),
    ).toThrow("At most 1 TV");
    expect(() =>
      account.save(
        state => withDevice(state, deviceId, newTv().keys.publicKey, [account.profiles[0]], "TV", 1000),
        { newDevices: [deviceId], deviceLimit: 1000 },
      ),
    ).not.toThrow();
  });

  it("keeps working for an account given fewer TVs than it has", () => {
    const account = setup();
    for (const name of ["One", "Two", "Three"]) signIn(account, newTv(name), [account.profiles[0]]);
    account.save(state => withList(state, list("Another"), [account.profiles[0]]).state, { deviceLimit: 2 });
    const tv = account.open().devices[0];
    account.save(state => withoutDevice(state, tv.id), { deviceLimit: 2 });
    expect(account.open().devices).toHaveLength(2);
    expect(() => signIn(account, newTv("Four"), [account.profiles[0]], false, 2)).toThrow(/at most 2 TVs/);
  });

  it("needs at least one profile", () => {
    const account = setup();
    expect(() => withDevice(account.open(), "d", newTv().keys.publicKey, [], "TV")).toThrow(
      /at least one profile/,
    );
  });
});

const signedIn = () => {
  const account = setup();
  const tv = newTv();
  const [living, kids] = account.profiles;
  return { account, ...signIn(account, tv, [living, kids]), living, kids };
};

describe("a TV's pulls", () => {
  it("pick up a list added to one of its profiles, opening only what changed", () => {
    const { account, deviceId, trust, kids } = signedIn();
    const first = openPull(trust, pull(account, deviceId));
    account.save(state => withList(state, list("Cartoons"), [kids]).state);
    const second = openPull(trust, pull(account, deviceId), first);
    expect(second.rev).toBe(first.rev + 1);
    expect(second.lists.map(item => item.name)).toEqual(["Living room list", "Kids list", "Cartoons"]);
    expect(second.profiles.find(profile => profile.id === kids)?.lists).toHaveLength(2);
    // Unchanged lists are the same objects as before: not opened again.
    expect(second.lists[0]).toBe(first.lists[0]);
  });

  it("bring each profile's colour and PIN, and their changes", () => {
    const { account, deviceId, trust, living, kids } = signedIn();
    const first = openPull(trust, pull(account, deviceId));
    expect(first.profiles.map(profile => profile.color)).toEqual([0, 1]);
    expect(first.profiles.some(profile => "pin" in profile)).toBe(false);
    account.save(state => withProfile(state, { name: "Kids", color: 7, pin: "1357" }, kids).state);
    const second = openPull(trust, pull(account, deviceId), first);
    expect(second.profiles.find(profile => profile.id === kids)).toMatchObject({ color: 7, pin: "1357" });
    expect(second.profiles.find(profile => profile.id === living)).not.toHaveProperty("pin");
    account.save(state => withProfile(state, { name: "Kids", pin: null }, kids).state);
    const third = openPull(trust, pull(account, deviceId), second);
    expect(third.profiles.find(profile => profile.id === kids)).not.toHaveProperty("pin");
  });

  it("don't see a list given only to a profile the TV doesn't have", () => {
    const { account, deviceId, trust } = signedIn();
    account.save(state => withList(state, list("Guest only"), [account.profiles[2]]).state);
    expect(openPull(trust, pull(account, deviceId)).lists.map(item => item.name)).not.toContain("Guest only");
  });

  it("refuse a pull older than the last one", () => {
    const { account, deviceId, trust, kids } = signedIn();
    const old = pull(account, deviceId);
    account.save(state => withList(state, list("Newer"), [kids]).state);
    const latest = openPull(trust, pull(account, deviceId));
    expect(() => openPull(trust, old, latest)).toThrow(/older/);
  });

  it("refuse a manifest not signed by the owner", () => {
    const { account, deviceId, trust } = signedIn();
    const sent = pullFor(account.stored(), deviceId);
    if (!sent) throw new Error("No pull");
    const forged = { ...sent, manifest: signJson(newOwnerKeys().signing.secretKey, { anything: 1 }) };
    expect(() => openPull(trust, roundTrip(forged))).toThrow(/signature/);
  });

  it("refuse profile keys boxed by anyone but the owner", () => {
    const { account, deviceId, trust } = signedIn();
    expect(() =>
      openPull({ ...trust, boxPublicKey: toBase64(nacl.box.keyPair().publicKey) }, pull(account, deviceId)),
    ).toThrow(/expected sender/);
  });

  it("refuse a list held back at an old revision", () => {
    const { account, deviceId, trust, living } = signedIn();
    const old = pullFor(account.stored(), deviceId);
    account.save(state => {
      const opened = state.lists[0];
      return withList(state, { ...opened.record, name: "Renamed" }, [living], opened.record.id).state;
    });
    const latest = pullFor(account.stored(), deviceId);
    if (!old || !latest) throw new Error("No pull");
    expect(() => openPull(trust, roundTrip({ ...latest, lists: old.lists }))).toThrow(/revision/);
  });

  it("lose a deleted profile, and its lists' new keys", () => {
    const { account, deviceId, trust, living, kids } = signedIn();
    const first = openPull(trust, pull(account, deviceId));
    account.save(state => withoutProfile(state, living));
    const after = openPull(trust, pull(account, deviceId), first);
    expect(after.profiles.map(profile => profile.id)).toEqual([kids]);
    expect(after.lists.map(item => item.name)).toEqual(["Kids list"]);
    expect(account.stored().devices[0].keys.map(key => key.profile)).toEqual([kids]);
  });

  it("say the TV was removed when the manifest no longer names it", () => {
    const { account, deviceId, trust } = signedIn();
    const sent = pull(account, deviceId);
    account.save(state => withoutDevice(state, deviceId));
    expect(pullFor(account.stored(), deviceId)).toBeNull();
    // A server that keeps sending the old pull is caught by the revision check instead.
    const view: DeviceView = openPull(trust, sent);
    expect(view.profiles).toHaveLength(2);
    // The current manifest, with this TV gone.
    const current = { ...JSON.parse(JSON.stringify(sent)), manifest: account.stored().manifest };
    expect(() => openPull(trust, current)).toThrow(DeviceRemoved);
  });
});

// --- Phase 6: changing a TV's profiles, and removing TVs ---

/** The keys a TV holds after a pull: its profile keys, and the list keys they open. */
const keysHeld = (trust: DeviceTrust, sent: unknown) => {
  const body: {
    keys: { profile: string; key: Sealed }[];
    lists: { id: string; keys: { profile: string; key: Sealed }[] }[];
  } = JSON.parse(JSON.stringify(sent));
  const profiles = new Map(
    body.keys.map(entry => [
      entry.profile,
      openBoxedKey(fromBase64(trust.boxPublicKey), fromBase64(trust.secretKey), entry.key),
    ]),
  );
  const lists = new Map<string, Uint8Array>();
  for (const item of body.lists)
    for (const entry of item.keys) {
      const profileKey = profiles.get(entry.profile);
      if (profileKey) lists.set(item.id, unwrapKey(profileKey, entry.key));
    }
  return { profiles, lists };
};

/** Two TVs: the first with Living room and Kids, the second with Kids only. */
const twoTvs = () => {
  const first = signedIn();
  const second = signIn(first.account, newTv("Kids TV"), [first.kids]);
  return { ...first, second };
};

/** Whether keys a TV kept still open what the account holds now. */
const opens = (key: Uint8Array | undefined, sealed: Sealed | undefined) => {
  if (!key || !sealed) return false;
  try {
    openJson(key, sealed);
    return true;
  } catch {
    return false;
  }
};

describe("a TV that loses a profile", () => {
  it("can't open the profile's or its lists' new versions; the TV that keeps it carries on", () => {
    const { account, deviceId, trust, living, kids, second } = twoTvs();
    const kept = keysHeld(trust, pull(account, deviceId));
    const secondBefore = openPull(second.trust, pull(account, second.deviceId));

    account.save(state => withDeviceProfiles(state, deviceId, [living]));
    // A new version of the Kids list, after.
    account.save(state => {
      const kidsList = state.lists.find(item => item.record.name === "Kids list");
      return withList(state, { ...list("Kids list, renamed") }, [kids], kidsList?.record.id).state;
    });

    const stored = account.stored();
    const kidsProfile = stored.profiles.find(profile => profile.id === kids);
    const kidsList = stored.lists.find(item => item.profileKeys.some(key => key.profile === kids));
    expect(opens(kept.profiles.get(kids), kidsProfile?.sealed)).toBe(false);
    expect(opens(kept.lists.get(kidsList?.id ?? ""), kidsList?.sealed)).toBe(false);
    // Its own pull has only Living room now.
    const view = openPull(trust, pull(account, deviceId));
    expect(view.profiles.map(profile => profile.name)).toEqual(["Living room"]);
    expect(view.lists.map(item => item.name)).toEqual(["Living room list"]);
    // The other TV has Kids' new key, and the new version.
    const secondAfter = openPull(second.trust, pull(account, second.deviceId), secondBefore);
    expect(secondAfter.lists.map(item => item.name)).toEqual(["Kids list, renamed"]);
  });

  it("can be given it back, or another one", () => {
    const { account, deviceId, trust, living } = signedIn();
    account.save(state => withDeviceProfiles(state, deviceId, [living]));
    account.save(state => withDeviceProfiles(state, deviceId, [living, account.profiles[2]]));
    const view = openPull(trust, pull(account, deviceId));
    expect(view.profiles.map(profile => profile.name)).toEqual(["Living room", "Guest"]);
    expect(view.lists.map(item => item.name)).toEqual(["Living room list", "Guest list"]);
  });

  it("must keep at least one", () => {
    const { account, deviceId } = signedIn();
    expect(() => withDeviceProfiles(account.open(), deviceId, [])).toThrow(/at least one profile/);
  });
});

describe("a removed TV", () => {
  it("can't read a new version, and is told it was removed; the others carry on", () => {
    const { account, deviceId, trust, kids, second } = twoTvs();
    const sent = pull(account, deviceId);
    const kept = keysHeld(trust, sent);
    account.save(state => withoutDevice(state, deviceId));
    expect(pullFor(account.stored(), deviceId)).toBeNull();
    const current = { ...JSON.parse(JSON.stringify(sent)), manifest: account.stored().manifest };
    expect(() => openPull(trust, current)).toThrow(DeviceRemoved);

    const stored = account.stored();
    for (const profile of stored.profiles)
      expect(opens(kept.profiles.get(profile.id), profile.sealed)).toBe(false);
    for (const item of stored.lists) expect(opens(kept.lists.get(item.id), item.sealed)).toBe(false);
    const view = openPull(second.trust, pull(account, second.deviceId));
    expect(view.profiles.map(profile => profile.id)).toEqual([kids]);
    expect(view.lists.map(item => item.name)).toEqual(["Kids list"]);
  });
});

/** The state as it was for a TV signed in before Phase 6: no public key in the manifest. */
const old = (state: OpenedState): OpenedState => ({
  ...state,
  devices: state.devices.map(({ publicKey: _key, ...device }) => device),
});

describe("the server, when a TV loses access", () => {
  it("refuses a change that doesn't give the profile and its lists new keys", () => {
    const { account, deviceId, living } = signedIn();
    expect(() =>
      account.save(state => ({
        ...state,
        devices: state.devices.map(device =>
          device.id === deviceId ? { ...device, profiles: [living] } : device,
        ),
      })),
    ).toThrow(/needs a new key/);
  });

  it("refuses new keys not sent to the TVs that keep the profile", () => {
    const { account, deviceId, second } = twoTvs();
    const before = account.open();
    const change = buildChange(account.owner, before, withoutDevice(before, deviceId));
    const withheld = {
      ...change,
      putDevices: change.putDevices.filter(device => device.id !== second.deviceId),
    };
    expect(() => account.server(withheld)).toThrow(/needs profile .* new key/);
  });

  it("can't give a TV signed in before Phase 6 a new key, but can remove it", () => {
    const { account, deviceId, living, kids } = signedIn();
    expect(() =>
      buildChange(
        account.owner,
        old(account.open()),
        withDeviceProfiles(old(account.open()), deviceId, [living, kids, account.profiles[2]]),
      ),
    ).toThrow(/signing in again/);
    account.save(state => withoutDevice(old(state), deviceId));
    expect(account.open().devices).toEqual([]);
  });
});
