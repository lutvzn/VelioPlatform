// A TV simulated with sync-core, as VelioTV will run it, and the phone
// signing it in as vault.js does: shared by the pairing and closing tests.

import {
  buildChange,
  fromBase64,
  isSealed,
  newOwnerKeys,
  newPairingSecrets,
  openDeviceInfo,
  openPairingPayload,
  openPull,
  ownerPublicKeys,
  pairingCode,
  pairingProof,
  pairingUrl,
  pairRequest,
  readEvent,
  readPairingCode,
  sealPairingPayload,
  toBase64,
  withDevice,
  withList,
  withProfile,
  type DeviceTrust,
  type ListDraft,
  type PairingKind,
  type ReadCode,
} from "@velio/sync-core";
import { expect } from "vitest";
import {
  fresh,
  openPage,
  post,
  save,
  SITE,
  signedInAccount,
  unescape,
  worker,
  type Browser,
} from "./helpers.ts";

export const list = (name: string): ListDraft => ({
  name,
  kind: "m3u",
  url: `http://${name.toLowerCase().replaceAll(" ", "-")}.example.invalid/list.m3u`,
  refreshHours: 24,
});

// --- The TV ---

export const tvFetch = (ip: string, path: string, init: RequestInit = {}) => {
  const headers = new Headers(init.headers);
  headers.set("cf-connecting-ip", ip);
  return worker.fetch(`${SITE}${path}`, { ...init, headers });
};

/** A box key pair, as the TV makes one. */
export const boxKeys = () => newOwnerKeys().box;

/** A TV that has asked to sign in: its keys, secrets and the pairing the server gave it. */
export const newTv = async (name = "Living room TV") => {
  const { ip } = fresh();
  const keys = boxKeys();
  const secrets = newPairingSecrets();
  const publicKey = toBase64(keys.publicKey);
  const started = await tvFetch(ip, "/v1/pair", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(pairRequest(secrets, { format: 1, publicKey, name })),
  });
  expect(started.status).toBe(201);
  const { id, pollToken } = await started.json<{ id: string; pollToken: string }>();
  const poll = (wait = 0) =>
    tvFetch(ip, `/v1/pair/${id}?wait=${wait}`, { headers: { Authorization: `Bearer ${pollToken}` } });
  return { ip, keys, secrets, publicKey, id, poll, url: pairingUrl(SITE, id, secrets) };
};
export type Tv = Awaited<ReturnType<typeof newTv>>;

/** The TV picks up the sign-in and keeps what it needs to trust pulls. */
export const pickUp = async (tv: Tv) => {
  const answer = await tv.poll();
  expect(answer.status).toBe(200);
  const done = await answer.json<{
    status: string;
    deviceId: string;
    deviceToken: string;
    kind: PairingKind;
    payload: unknown;
  }>();
  expect(done.status).toBe("done");
  if (!isSealed(done.payload)) throw new Error("No payload");
  const payload = openPairingPayload(done.kind, tv.secrets, done.payload, tv.publicKey);
  expect(payload.device).toBe(done.deviceId);
  const trust: DeviceTrust = {
    account: payload.account,
    device: payload.device,
    secretKey: toBase64(tv.keys.secretKey),
    signPublicKey: payload.signPublicKey,
    boxPublicKey: payload.boxPublicKey,
  };
  return { trust, token: done.deviceToken };
};

export const pull = (tv: Tv, token: string, rev?: number) =>
  tvFetch(tv.ip, "/v1/sync", {
    headers: {
      Authorization: `Bearer ${token}`,
      ...(rev === undefined ? {} : { "If-None-Match": `"${rev}"` }),
    },
  });

// --- The phone ---

export const attribute = (html: string, name: string) => {
  const match = new RegExp(`${name}="([^"]*)"`).exec(html);
  if (!match?.[1]) throw new Error(`No ${name} in the answer`);
  return unescape(match[1]);
};

export const codeFor = (tv: Tv, kind: PairingKind): ReadCode => {
  const code = readPairingCode(kind === "qr" ? tv.url : pairingCode(tv.id, tv.secrets));
  if (!code) throw new Error("Unreadable code");
  return code;
};

export const find = (browser: Browser, code: ReadCode) =>
  post(browser.visitor, "/pair/find", {
    id: code.id,
    kind: code.kind,
    proof: pairingProof(code.kind, code.secret),
  });

/** What vault.js does on /pair: find the TV, open its details, and send the change and payload. */
export const signInTv = async (browser: Browser, tv: Tv, profileIds: string[], kind: PairingKind = "qr") => {
  const code = codeFor(tv, kind);
  const found = await find(browser, code);
  expect(found.status).toBe(200);
  const html = await found.text();
  const deviceId = attribute(html, "data-device-id");
  const info = openDeviceInfo(code, JSON.parse(attribute(html, "data-sealed-device")));
  expect(info.publicKey).toBe(tv.publicKey);
  const { state } = await openPage(browser, "/pair");
  const after = withDevice(state, deviceId, fromBase64(info.publicKey), profileIds, info.name);
  const payload = sealPairingPayload(code, {
    format: 1,
    account: browser.accountId,
    device: deviceId,
    devicePublicKey: info.publicKey,
    ...ownerPublicKeys(browser.owner),
  });
  return post(browser.visitor, `/pair/${code.id}`, {
    change: JSON.stringify(buildChange(browser.owner, state, after)),
    payload: JSON.stringify(payload),
    kind: code.kind,
    proof: pairingProof(code.kind, code.secret),
  });
};

/** An account with three profiles, a list for each, and one shared by the first two. */
export const threeProfiles = async () => {
  const account = await signedInAccount();
  const ids: string[] = [];
  await save(account.browser, state => {
    let next = state;
    for (const name of ["Living room", "Kids", "Guest"]) {
      const made = withProfile(next, { name });
      ids.push(made.profileId);
      next = made.state;
    }
    next = withList(next, list("Living room list"), [ids[0]]).state;
    next = withList(next, list("Kids list"), [ids[1]]).state;
    next = withList(next, list("Guest list"), [ids[2]]).state;
    return withList(next, list("Shared list"), [ids[0], ids[1]]).state;
  });
  return { ...account, ids };
};

/** Opens the TV's socket and records what arrives. */
export const connect = async (tv: Tv, protocols: string[] | null) => {
  const headers: Record<string, string> = { Upgrade: "websocket" };
  if (protocols) headers["Sec-WebSocket-Protocol"] = protocols.join(", ");
  const answer = await tvFetch(tv.ip, "/v1/events", { headers });
  const ws = answer.webSocket;
  const messages: string[] = [];
  const closes: number[] = [];
  if (ws) {
    ws.accept();
    ws.addEventListener("message", event => {
      messages.push(String(event.data));
    });
    ws.addEventListener("close", event => {
      closes.push(event.code);
    });
  }
  const revs = () => messages.map(readEvent).flatMap(event => (event?.type === "rev" ? [event.rev] : []));
  return { answer, ws, messages, closes, revs };
};

export const signedInTv = async () => {
  const account = await threeProfiles();
  const tv = await newTv();
  await signInTv(account.browser, tv, [account.ids[0], account.ids[1]]);
  const { trust, token } = await pickUp(tv);
  const view = openPull(trust, await (await pull(tv, token)).json());
  return { ...account, tv, token, trust, view };
};
