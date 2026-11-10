// The per-account Durable Object (PLAN.md, "Server", "Data"): the owner
// bundle, the signed manifest, and the account's profiles and lists, all
// ciphertext. One object per account, so changes are strictly ordered. It
// also holds the TVs' WebSockets (Phase 5), with hibernation, and sends
// "rev N" after each change (sync-core events.ts). The Worker reaches it by
// account id: env.ACCOUNT.get(env.ACCOUNT.idFromName(accountId)).

import { DurableObject } from "cloudflare:workers";
import {
  applyChange,
  ChangeProblem,
  EVENTS_CLOSE,
  EVENTS_PROTOCOL,
  fromBase64,
  PING,
  PONG,
  REMOVED_EVENT,
  revEvent,
  isSealed,
  pullFor,
  type ChangeOptions,
  type ChangeProblemCode,
  type OwnerPublicKeys,
  type Sealed,
  type StoredDevice,
  type StoredList,
  type StoredProfile,
  type StoredState,
  type SyncPull,
} from "@velio/sync-core";
import { sameDigest } from "./lib/tokens.ts";

export interface OwnerRecord extends OwnerPublicKeys {
  /** The owner bundle, sealed under the unlock key from the password. */
  bundle: Sealed;
  updatedAt: number;
}

/** A TV's pull: what it may read, the same revision it has, or nothing, as the account no longer names it. */
export type PullResult =
  | { status: "pull"; rev: number; pull: SyncPull }
  | { status: "same"; rev: number }
  | { status: "removed" };

export type ChangeResult =
  /** removedDevices: TVs the change signed out, whose tokens the Worker deletes from D1. */
  | { ok: true; rev: number; removedDevices: string[] }
  | { ok: false; code: ChangeProblemCode; message: string };

const SCHEMA = `
  CREATE TABLE IF NOT EXISTS owner (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    bundle TEXT NOT NULL,
    sign_public_key TEXT NOT NULL,
    box_public_key TEXT NOT NULL,
    updated_at INTEGER NOT NULL
  );
  CREATE TABLE IF NOT EXISTS recovery (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    -- SHA-256 of the recovery key's auth key, in hex.
    auth_hash TEXT NOT NULL,
    -- The owner bundle again, sealed under the recovery key's unlock key.
    bundle TEXT NOT NULL,
    updated_at INTEGER NOT NULL
  );
  CREATE TABLE IF NOT EXISTS manifest (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    rev INTEGER NOT NULL,
    signed TEXT NOT NULL,
    updated_at INTEGER NOT NULL
  );
  CREATE TABLE IF NOT EXISTS profiles (
    id TEXT PRIMARY KEY,
    rev INTEGER NOT NULL,
    sealed TEXT NOT NULL,
    key TEXT NOT NULL,
    position INTEGER NOT NULL
  );
  CREATE TABLE IF NOT EXISTS lists (
    id TEXT PRIMARY KEY,
    rev INTEGER NOT NULL,
    key_rev INTEGER NOT NULL,
    sealed TEXT NOT NULL,
    key TEXT NOT NULL,
    position INTEGER NOT NULL
  );
  CREATE TABLE IF NOT EXISTS list_keys (
    list_id TEXT NOT NULL,
    profile_id TEXT NOT NULL,
    key TEXT NOT NULL,
    PRIMARY KEY (list_id, profile_id)
  );
  CREATE TABLE IF NOT EXISTS devices (
    id TEXT PRIMARY KEY,
    position INTEGER NOT NULL
  );
  CREATE TABLE IF NOT EXISTS device_keys (
    device_id TEXT NOT NULL,
    profile_id TEXT NOT NULL,
    key TEXT NOT NULL,
    PRIMARY KEY (device_id, profile_id)
  );`;

/** How the Worker tells the object which TV a socket is for, once it has checked the token. */
export const EVENTS_DEVICE_HEADER = "X-Velio-Device";

/** Sockets one TV may hold at once: a reconnect can race the old socket's end. */
const SOCKETS_PER_DEVICE = 2;

interface SocketAttachment {
  device: string;
  connectedAt: number;
}

const attachmentOf = (ws: WebSocket): SocketAttachment | undefined => {
  const value: unknown = ws.deserializeAttachment();
  if (typeof value !== "object" || value === null || !("device" in value) || !("connectedAt" in value))
    return undefined;
  const { device, connectedAt } = value;
  return typeof device === "string" && typeof connectedAt === "number" ? { device, connectedAt } : undefined;
};

/** Sends, ignoring a socket that is already closing. */
const trySend = (ws: WebSocket, message: string) => {
  try {
    ws.send(message);
  } catch {
    // It closed between listing and sending; its TV reconnects or polls.
  }
};

const tryClose = (ws: WebSocket, code: number, reason: string) => {
  try {
    ws.close(code, reason);
  } catch {
    // Already closed.
  }
};

const closeRemoved = (ws: WebSocket) => {
  trySend(ws, REMOVED_EVENT);
  tryClose(ws, EVENTS_CLOSE.removed, "removed");
};

const readJsonSealed = (json: string): Sealed => {
  const value: unknown = JSON.parse(json);
  if (!isSealed(value)) throw new Error("A stored record isn't readable");
  return value;
};

export class Account extends DurableObject<Env> {
  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    this.ctx.storage.sql.exec(SCHEMA);
    // Answered by the runtime, without waking the object or billing its time.
    this.ctx.setWebSocketAutoResponse(new WebSocketRequestResponsePair(PING, PONG));
  }

  /**
   * GET /v1/events, forwarded by the Worker once it has found the TV by its
   * token: accepts the socket with hibernation and sends the current revision.
   */
  override async fetch(request: Request): Promise<Response> {
    if (request.headers.get("Upgrade")?.toLowerCase() !== "websocket")
      return new Response("This address takes a WebSocket\n", { status: 426 });
    const deviceId = request.headers.get(EVENTS_DEVICE_HEADER) ?? "";
    const sql = this.ctx.storage.sql;
    const listed = sql.exec("SELECT 1 FROM devices WHERE id = ?", deviceId).toArray().length > 0;
    const manifest = sql.exec<{ rev: number }>("SELECT rev FROM manifest WHERE id = 1").toArray()[0];
    if (!listed || !manifest) return new Response("This TV was removed from the account\n", { status: 401 });

    // Keep this TV's newest sockets only; an older one is likely dead already.
    const older = this.ctx
      .getWebSockets(deviceId)
      .map(ws => ({ ws, at: attachmentOf(ws)?.connectedAt ?? 0 }))
      .toSorted((a, b) => a.at - b.at);
    for (const { ws } of older.slice(0, Math.max(0, older.length - (SOCKETS_PER_DEVICE - 1))))
      tryClose(ws, EVENTS_CLOSE.replaced, "replaced");

    const { 0: client, 1: server } = new WebSocketPair();
    this.ctx.acceptWebSocket(server, [deviceId]);
    server.serializeAttachment({ device: deviceId, connectedAt: Date.now() } satisfies SocketAttachment);
    // What the TV missed while it had no socket.
    server.send(revEvent(manifest.rev));
    return new Response(null, {
      status: 101,
      webSocket: client,
      headers: { "Sec-WebSocket-Protocol": EVENTS_PROTOCOL },
    });
  }

  /** TVs only send "ping", which the runtime answers; anything else is ignored. */
  override webSocketMessage() {}

  /** Answers the TV's close; harmless where the runtime has already answered it. */
  override webSocketClose(ws: WebSocket, code: number, reason: string) {
    // 1005 and 1006 can't be sent back.
    tryClose(ws, code === 1005 || code === 1006 ? 1000 : code, reason);
  }

  override webSocketError() {}

  /** Open sockets, for tests and the device page. */
  socketCount(deviceId?: string) {
    return this.ctx.getWebSockets(deviceId).length;
  }

  /** Tells every TV the new revision, and closes the sockets of TVs the manifest no longer names. */
  private notify(rev: number, devices: string[]) {
    for (const ws of this.ctx.getWebSockets()) {
      const attachment = attachmentOf(ws);
      if (!attachment || !devices.includes(attachment.device)) closeRemoved(ws);
      else trySend(ws, revEvent(rev));
    }
  }

  setOwner(owner: OwnerRecord) {
    this.ctx.storage.sql.exec(
      `INSERT INTO owner (id, bundle, sign_public_key, box_public_key, updated_at) VALUES (1, ?, ?, ?, ?)
       ON CONFLICT (id) DO UPDATE SET bundle = excluded.bundle, sign_public_key = excluded.sign_public_key,
         box_public_key = excluded.box_public_key, updated_at = excluded.updated_at`,
      JSON.stringify(owner.bundle),
      owner.signPublicKey,
      owner.boxPublicKey,
      owner.updatedAt,
    );
  }

  /** Turns the recovery key on, or replaces it, with its hash and copy of the owner bundle; null turns it off. */
  setRecovery(recovery: { authHash: string; bundle: Sealed } | null) {
    const sql = this.ctx.storage.sql;
    if (!recovery) {
      sql.exec("DELETE FROM recovery");
      return;
    }
    sql.exec(
      `INSERT INTO recovery (id, auth_hash, bundle, updated_at) VALUES (1, ?, ?, ?)
       ON CONFLICT (id) DO UPDATE SET auth_hash = excluded.auth_hash, bundle = excluded.bundle,
         updated_at = excluded.updated_at`,
      recovery.authHash,
      JSON.stringify(recovery.bundle),
      Date.now(),
    );
  }

  /** When the recovery key was made, or null when it is off. */
  recoveryMadeAt(): number | null {
    const row = this.ctx.storage.sql
      .exec<{ updated_at: number }>("SELECT updated_at FROM recovery WHERE id = 1")
      .toArray()[0];
    return row?.updated_at ?? null;
  }

  /** The recovery copy of the owner bundle, with the public keys, for the right recovery auth key's hash. */
  recoveryBundle(authHash: string): OwnerRecord | null {
    const row = this.ctx.storage.sql
      .exec<{ auth_hash: string; bundle: string }>("SELECT auth_hash, bundle FROM recovery WHERE id = 1")
      .toArray()[0];
    const owner = this.getOwner();
    if (!row || !owner || !sameDigest(row.auth_hash, authHash)) return null;
    return { ...owner, bundle: readJsonSealed(row.bundle) };
  }

  getOwner(): OwnerRecord | null {
    const row = this.ctx.storage.sql
      .exec<{ bundle: string; sign_public_key: string; box_public_key: string; updated_at: number }>(
        "SELECT bundle, sign_public_key, box_public_key, updated_at FROM owner WHERE id = 1",
      )
      .toArray()[0];
    if (!row) return null;
    return {
      bundle: readJsonSealed(row.bundle),
      signPublicKey: row.sign_public_key,
      boxPublicKey: row.box_public_key,
      updatedAt: row.updated_at,
    };
  }

  /** Everything the owner's device opens: the manifest, the profiles and the lists, in the order they were made. */
  getState(): StoredState {
    const sql = this.ctx.storage.sql;
    const manifest = sql.exec<{ signed: string }>("SELECT signed FROM manifest WHERE id = 1").toArray()[0];
    const profiles = sql
      .exec<{ id: string; rev: number; sealed: string; key: string }>(
        "SELECT id, rev, sealed, key FROM profiles ORDER BY position",
      )
      .toArray()
      .map((row): StoredProfile => ({
        id: row.id,
        rev: row.rev,
        sealed: readJsonSealed(row.sealed),
        key: readJsonSealed(row.key),
      }));
    const keys = new Map<string, StoredList["profileKeys"]>();
    for (const row of sql
      .exec<{ list_id: string; profile_id: string; key: string }>(
        `SELECT k.list_id, k.profile_id, k.key FROM list_keys k JOIN profiles p ON p.id = k.profile_id
         ORDER BY p.position`,
      )
      .toArray()) {
      const entries = keys.get(row.list_id) ?? [];
      entries.push({ profile: row.profile_id, key: readJsonSealed(row.key) });
      keys.set(row.list_id, entries);
    }
    const lists = sql
      .exec<{ id: string; rev: number; key_rev: number; sealed: string; key: string }>(
        "SELECT id, rev, key_rev, sealed, key FROM lists ORDER BY position",
      )
      .toArray()
      .map((row): StoredList => ({
        id: row.id,
        rev: row.rev,
        keyRev: row.key_rev,
        sealed: readJsonSealed(row.sealed),
        key: readJsonSealed(row.key),
        profileKeys: keys.get(row.id) ?? [],
      }));
    const deviceKeys = new Map<string, StoredDevice["keys"]>();
    for (const row of sql
      .exec<{ device_id: string; profile_id: string; key: string }>(
        `SELECT k.device_id, k.profile_id, k.key FROM device_keys k JOIN profiles p ON p.id = k.profile_id
         ORDER BY p.position`,
      )
      .toArray()) {
      const entries = deviceKeys.get(row.device_id) ?? [];
      entries.push({ profile: row.profile_id, key: readJsonSealed(row.key) });
      deviceKeys.set(row.device_id, entries);
    }
    const devices = sql
      .exec<{ id: string }>("SELECT id FROM devices ORDER BY position")
      .toArray()
      .map((row): StoredDevice => ({ id: row.id, keys: deviceKeys.get(row.id) ?? [] }));
    if (!manifest) return { manifest: null, profiles, lists, devices };
    const signed: unknown = JSON.parse(manifest.signed);
    if (
      typeof signed !== "object" ||
      signed === null ||
      !("body" in signed) ||
      !("signature" in signed) ||
      typeof signed.body !== "string" ||
      typeof signed.signature !== "string"
    )
      throw new Error("The stored manifest isn't readable");
    return { manifest: { body: signed.body, signature: signed.signature }, profiles, lists, devices };
  }

  /**
   * Applies one change from the owner (sync-core's applyChange), all or
   * nothing. Answers "behind" when another tab or device changed the account
   * first.
   */
  change(accountId: string, input: unknown, options: ChangeOptions = {}): ChangeResult {
    const owner = this.getOwner();
    if (!owner) return { ok: false, code: "refused", message: "The account has no owner keys" };
    let result: ReturnType<typeof applyChange>;
    try {
      result = applyChange(this.getState(), input, accountId, fromBase64(owner.signPublicKey), options);
    } catch (error) {
      if (error instanceof ChangeProblem) return { ok: false, code: error.code, message: error.message };
      throw error;
    }
    const { change, manifest } = result;
    const sql = this.ctx.storage.sql;
    const now = Date.now();
    this.ctx.storage.transactionSync(() => {
      const nextPosition = (table: "profiles" | "lists" | "devices") =>
        sql.exec<{ next: number }>(`SELECT COALESCE(MAX(position), 0) + 1 AS next FROM ${table}`).one().next;
      for (const profileId of change.deleteProfiles) {
        sql.exec("DELETE FROM profiles WHERE id = ?", profileId);
        sql.exec("DELETE FROM list_keys WHERE profile_id = ?", profileId);
        sql.exec("DELETE FROM device_keys WHERE profile_id = ?", profileId);
      }
      for (const profile of change.putProfiles)
        sql.exec(
          `INSERT INTO profiles (id, rev, sealed, key, position) VALUES (?, ?, ?, ?, ?)
           ON CONFLICT (id) DO UPDATE SET rev = excluded.rev, sealed = excluded.sealed, key = excluded.key`,
          profile.id,
          profile.rev,
          JSON.stringify(profile.sealed),
          JSON.stringify(profile.key),
          nextPosition("profiles"),
        );
      for (const listId of change.deleteLists) {
        sql.exec("DELETE FROM lists WHERE id = ?", listId);
        sql.exec("DELETE FROM list_keys WHERE list_id = ?", listId);
      }
      for (const list of change.putLists) {
        sql.exec(
          `INSERT INTO lists (id, rev, key_rev, sealed, key, position) VALUES (?, ?, ?, ?, ?, ?)
           ON CONFLICT (id) DO UPDATE SET rev = excluded.rev, key_rev = excluded.key_rev, sealed = excluded.sealed,
             key = excluded.key`,
          list.id,
          list.rev,
          list.keyRev,
          JSON.stringify(list.sealed),
          JSON.stringify(list.key),
          nextPosition("lists"),
        );
        sql.exec("DELETE FROM list_keys WHERE list_id = ?", list.id);
        for (const entry of list.profileKeys)
          sql.exec(
            "INSERT INTO list_keys (list_id, profile_id, key) VALUES (?, ?, ?)",
            list.id,
            entry.profile,
            JSON.stringify(entry.key),
          );
      }
      for (const deviceId of change.deleteDevices) {
        sql.exec("DELETE FROM devices WHERE id = ?", deviceId);
        sql.exec("DELETE FROM device_keys WHERE device_id = ?", deviceId);
      }
      for (const device of change.putDevices) {
        sql.exec(
          "INSERT INTO devices (id, position) VALUES (?, ?) ON CONFLICT (id) DO NOTHING",
          device.id,
          nextPosition("devices"),
        );
        sql.exec("DELETE FROM device_keys WHERE device_id = ?", device.id);
        for (const entry of device.keys)
          sql.exec(
            "INSERT INTO device_keys (device_id, profile_id, key) VALUES (?, ?, ?)",
            device.id,
            entry.profile,
            JSON.stringify(entry.key),
          );
      }
      // A TV a profile was taken from isn't always in putDevices: drop the keys the manifest no longer gives it.
      for (const entry of manifest.devices)
        sql.exec(
          "DELETE FROM device_keys WHERE device_id = ? AND profile_id NOT IN (SELECT value FROM json_each(?))",
          entry.id,
          JSON.stringify(entry.profiles),
        );
      sql.exec(
        `INSERT INTO manifest (id, rev, signed, updated_at) VALUES (1, ?, ?, ?)
         ON CONFLICT (id) DO UPDATE SET rev = excluded.rev, signed = excluded.signed, updated_at = excluded.updated_at`,
        manifest.rev,
        JSON.stringify(change.manifest),
        now,
      );
    });
    this.notify(
      manifest.rev,
      manifest.devices.map(device => device.id),
    );
    return { ok: true, rev: manifest.rev, removedDevices: change.deleteDevices };
  }

  /** What one TV may read, unless it already has revision `knownRev`. */
  devicePull(deviceId: string, knownRev?: number): PullResult {
    const sql = this.ctx.storage.sql;
    const listed = sql.exec("SELECT 1 FROM devices WHERE id = ?", deviceId).toArray().length > 0;
    const manifest = sql.exec<{ rev: number }>("SELECT rev FROM manifest WHERE id = 1").toArray()[0];
    if (!listed || !manifest) return { status: "removed" };
    if (knownRev === manifest.rev) return { status: "same", rev: manifest.rev };
    const pull = pullFor(this.getState(), deviceId);
    return pull ? { status: "pull", rev: manifest.rev, pull } : { status: "removed" };
  }

  /** Closes one TV's sockets: it signed itself out (POST /v1/signout). */
  async closeDevice(deviceId: string) {
    for (const ws of this.ctx.getWebSockets(deviceId)) closeRemoved(ws);
  }

  /** Signs out every TV that has a socket open, keeping the data: the account was closed. */
  async closeSockets() {
    for (const ws of this.ctx.getWebSockets()) closeRemoved(ws);
  }

  /** Deletes everything the account holds (a reset, an account deleted, or a closed one's data after 30 days). */
  async reset() {
    for (const ws of this.ctx.getWebSockets()) closeRemoved(ws);
    await this.ctx.storage.deleteAll();
    this.ctx.storage.sql.exec(SCHEMA);
  }
}

export const accountStub = (env: Env, accountId: string) =>
  env.ACCOUNT.get(env.ACCOUNT.idFromName(accountId));
