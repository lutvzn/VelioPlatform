// Phase 3's server half, end to end (PLAN.md, Phases:
// "A TV signs in by QR with two of three profiles, and a list added to one of
// them imports within 15 minutes"). The TV is simulated with sync-core, as
// VelioTV will run it; the phone stands in for vault.js, as in vault.test.ts.

import { env } from "./support/runtime.ts";
import {
  buildChange,
  EVENTS_CLOSE,
  EVENTS_PROTOCOL,
  eventsProtocols,
  fromBase64,
  newPairingSecrets,
  openPull,
  pairingProof,
  pairRequest,
  toBase64,
  withDevice,
  withDeviceProfiles,
  withList,
  withoutDevice,
  withProfile,
  type DeviceView,
} from "@velio/sync-core";
import { describe, expect, it, vi } from "vitest";
import { fresh, linkIn, newCredentials, openPage, post, request, save, send } from "./helpers.ts";
import {
  attribute,
  boxKeys,
  codeFor,
  connect,
  find,
  list,
  newTv,
  pickUp,
  pull,
  signedInTv,
  signInTv,
  threeProfiles,
  tvFetch,
} from "./tv.ts";

const names = (view: DeviceView) => view.lists.map(item => item.name).toSorted();

describe("signing in a TV", () => {
  it("by QR with two of three profiles; a list added to one of them reaches it on the next pull", async () => {
    const { browser, ids } = await threeProfiles();
    const tv = await newTv();
    const waiting = await tv.poll();
    expect(await waiting.json()).toMatchObject({ status: "waiting" });

    const signedIn = await signInTv(browser, tv, [ids[0], ids[1]]);
    expect(signedIn.status).toBe(200);
    const done = await signedIn.text();
    expect(done).toMatch(/<strong><span data-tv-name="[^"]+">…<\/span><\/strong> is signed in/);
    expect(done).not.toContain("Living room TV");

    const { trust, token } = await pickUp(tv);
    const first = await pull(tv, token);
    expect(first.status).toBe(200);
    const etag = first.headers.get("ETag");
    const view = openPull(trust, await first.json());
    expect(view.profiles.map(profile => profile.name)).toEqual(["Living room", "Kids"]);
    expect(names(view)).toEqual(["Kids list", "Living room list", "Shared list"]);
    expect(etag).toBe(`"${view.rev}"`);

    // Nothing new: 304.
    expect((await pull(tv, token, view.rev)).status).toBe(304);

    // A list added to Kids on the web.
    await save(browser, state => withList(state, list("New for kids"), [ids[1]]).state);
    const second = await pull(tv, token, view.rev);
    expect(second.status).toBe(200);
    const after = openPull(trust, await second.json(), view);
    expect(names(after)).toEqual(["Kids list", "Living room list", "New for kids", "Shared list"]);
    expect(after.profiles.find(profile => profile.name === "Kids")?.lists).toHaveLength(3);

    // The guest's list never reached the TV, even as ciphertext.
    const raw = JSON.stringify(await (await pull(tv, token)).json());
    const { state } = await openPage(browser);
    const guest = state.lists.find(item => item.record.name === "Guest list");
    expect(guest).toBeDefined();
    expect(raw).not.toContain(guest?.record.id);
    expect(raw).not.toContain(ids[2]);
  });

  it("by the typed code", async () => {
    const { browser, ids } = await threeProfiles();
    const tv = await newTv("Bedroom TV");
    expect((await signInTv(browser, tv, [ids[2]], "code")).status).toBe(200);
    const { trust, token } = await pickUp(tv);
    const view = openPull(trust, await (await pull(tv, token)).json());
    expect(names(view)).toEqual(["Guest list"]);
  });

  it("keeps the TV's name sealed for the owner: the server shows only how many TVs there are", async () => {
    const { browser, ids } = await threeProfiles();
    const tv = await newTv("Kitchen TV");
    await signInTv(browser, tv, [ids[0]]);
    const account = await (await request(browser.visitor, "/account")).text();
    expect(account).toContain("You have 1 TV.");
    const { html, state } = await openPage(browser, "/devices");
    expect(state.devices.map(device => device.name)).toEqual(["Kitchen TV"]);
    expect(html).not.toContain("Kitchen TV");
    expect(account).not.toContain("Kitchen TV");
  });
});

describe("pairing", () => {
  it("finds a TV only with the proof of its secret", async () => {
    const { browser } = await threeProfiles();
    const tv = await newTv();
    const code = codeFor(tv, "qr");
    expect((await find(browser, { ...code, secret: "A".repeat(22) })).status).toBe(404);
    expect((await find(browser, { ...code, kind: "code" })).status).toBe(404);
    expect((await post(browser.visitor, "/pair/find", { id: tv.id, kind: "qr", proof: "" })).status).toBe(
      404,
    );
    expect((await find(browser, code)).status).toBe(200);
    // Signed out: nothing.
    expect((await find({ ...browser, visitor: { ip: browser.visitor.ip } }, code)).status).toBe(401);
  });

  it("works once: a second phone can't take the TV, and the TV gets its token once", async () => {
    const first = await threeProfiles();
    const second = await threeProfiles();
    const tv = await newTv();
    expect((await signInTv(first.browser, tv, [first.ids[0]])).status).toBe(200);
    expect((await find(second.browser, codeFor(tv, "qr"))).status).toBe(404);
    await pickUp(tv);
    expect((await tv.poll()).status).toBe(410);
  });

  it("puts the claim back when the change is refused, so the owner can try again", async () => {
    const { browser, ids } = await threeProfiles();
    const tv = await newTv();
    const code = codeFor(tv, "qr");
    const found = await find(browser, code);
    const deviceId = attribute(await found.text(), "data-device-id");
    const { state } = await openPage(browser, "/pair");
    // A change built on an older page: behind.
    await save(browser, next => withProfile(next, { name: "Another" }).state);
    const stale = await post(browser.visitor, `/pair/${code.id}`, {
      change: JSON.stringify(
        buildChange(
          browser.owner,
          state,
          withDevice(state, deviceId, fromBase64(tv.publicKey), [ids[0]], "TV"),
        ),
      ),
      payload: JSON.stringify({ nonce: "x", box: "y" }),
      kind: code.kind,
      proof: pairingProof(code.kind, code.secret),
    });
    expect(stale.status).toBe(409);
    expect(stale.headers.get("HX-Retarget")).toBe("#pair-problem");
    expect((await signInTv(browser, tv, [ids[0]])).status).toBe(200);
  });

  it("refuses a TV added through /changes, without signing it in", async () => {
    const { browser, ids } = await threeProfiles();
    const { state } = await openPage(browser);
    const sneaky = withDevice(state, "made-up-device", boxKeys().publicKey, [ids[0]], "TV");
    expect((await send(browser, buildChange(browser.owner, state, sneaky))).status).toBe(400);
  });

  it("expires after 10 minutes", async () => {
    const { browser } = await threeProfiles();
    const tv = await newTv();
    await env.DB.prepare("UPDATE pairings SET expires_at = ? WHERE id = ?")
      .bind(Date.now() - 1, tv.id)
      .run();
    expect((await find(browser, codeFor(tv, "qr"))).status).toBe(404);
    expect((await tv.poll()).status).toBe(410);
  });

  it("refuses unreadable requests to start", async () => {
    const { ip } = fresh();
    const start = (body: unknown) =>
      tvFetch(ip, "/v1/pair", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      });
    const good = pairRequest(newPairingSecrets(), {
      format: 1,
      publicKey: toBase64(boxKeys().publicKey),
      name: "TV",
    });
    expect((await start(good)).status).toBe(201);
    expect((await start({ ...good, publicKey: "nope" })).status).toBe(400);
    // Old VelioTV builds also send the name in plain text: it's ignored.
    expect((await start({ ...good, name: "x".repeat(41) })).status).toBe(201);
    expect((await start({ ...good, proofs: undefined })).status).toBe(400);
    expect((await start({ ...good, sealed: { qr: good.sealed.qr } })).status).toBe(400);
    // A wrong poll token looks like no pairing.
    const { id } = await (await start(good)).json<{ id: string }>();
    const wrong = await tvFetch(ip, `/v1/pair/${id}?wait=0`, {
      headers: { Authorization: `Bearer ${"x".repeat(43)}` },
    });
    expect(wrong.status).toBe(404);
  });

  it("is a whole page when opened by a boosted link", async () => {
    const { browser } = await threeProfiles();
    const page = await request(browser.visitor, "/pair", { headers: { "HX-Request": "true" } });
    const html = await page.text();
    expect(html).toContain('<main id="main">');
    expect(html).toContain('data-velio="pair-find"');
  });

  it("asks a signed-out phone to sign in first, then shows the pairing page", async () => {
    const page = await request({ ip: "192.0.2.9" }, "/pair");
    expect(page.status).toBe(200);
    const html = await page.text();
    expect(html).toContain('name="next" value="/pair"');
    expect(html).toContain('data-velio="signin"');
  });
});

describe("pulling", () => {
  it("needs a TV's token", async () => {
    const { ip } = fresh();
    expect((await tvFetch(ip, "/v1/sync")).status).toBe(401);
    const wrong = await tvFetch(ip, "/v1/sync", { headers: { Authorization: `Bearer ${"x".repeat(43)}` } });
    expect(wrong.status).toBe(401);
  });

  it("stops after the account is reset", async () => {
    const { browser, ids, email } = await threeProfiles();
    const tv = await newTv();
    await signInTv(browser, tv, [ids[0]]);
    const { token } = await pickUp(tv);
    expect((await pull(tv, token)).status).toBe(200);

    expect((await post(browser.visitor, "/reset", { email })).status).toBe(200);
    const link = await linkIn(email, "reset");
    expect((await post(browser.visitor, `/reset/${link}`, newCredentials().fields)).status).toBe(200);
    expect((await pull(tv, token)).status).toBe(401);
    expect(
      await env.DB.prepare("SELECT id FROM devices WHERE account_id = ?").bind(browser.accountId).first(),
    ).toBeNull();
  });
});

// --- Live updates (Phase 5) ---

describe("live updates", () => {
  it("tell a TV the revision when it connects and after each change on the web", async () => {
    const { browser, ids, tv, token, view } = await signedInTv();
    const socket = await connect(tv, eventsProtocols(token));
    expect(socket.answer.status).toBe(101);
    expect(socket.answer.headers.get("Sec-WebSocket-Protocol")).toBe(EVENTS_PROTOCOL);
    await vi.waitFor(() => expect(socket.revs()).toEqual([view.rev]));

    await save(browser, state => withList(state, list("Live for kids"), [ids[1]]).state);
    await vi.waitFor(() => expect(socket.revs()).toEqual([view.rev, view.rev + 1]));
    // The TV pulls the revision it was told about.
    const after = await pull(tv, token, view.rev);
    expect(after.headers.get("ETag")).toBe(`"${view.rev + 1}"`);

    // Pings are answered by the runtime.
    socket.ws?.send("ping");
    await vi.waitFor(() => expect(socket.messages).toContain("pong"));
    socket.ws?.close(1000, "done");
  });

  it("need the TV's token as a subprotocol, beside the events protocol", async () => {
    const { tv, token } = await signedInTv();
    expect((await tvFetch(tv.ip, "/v1/events")).status).toBe(426);
    expect((await connect(tv, null)).answer.status).toBe(401);
    expect((await connect(tv, [`bearer.${token}`])).answer.status).toBe(401);
    expect((await connect(tv, eventsProtocols("x".repeat(43)))).answer.status).toBe(401);
    // Never in the address.
    const inQuery = await tvFetch(tv.ip, `/v1/events?token=${token}`, { headers: { Upgrade: "websocket" } });
    expect(inQuery.status).toBe(401);
  });

  it("close a reset account's sockets as removed", async () => {
    const { browser, email, tv, token } = await signedInTv();
    const socket = await connect(tv, eventsProtocols(token));
    await vi.waitFor(() => expect(socket.revs()).toHaveLength(1));
    expect((await post(browser.visitor, "/reset", { email })).status).toBe(200);
    const link = await linkIn(email, "reset");
    expect((await post(browser.visitor, `/reset/${link}`, newCredentials().fields)).status).toBe(200);
    await vi.waitFor(() => expect(socket.closes).toEqual([EVENTS_CLOSE.removed]));
    expect(socket.messages).toContain("removed");
    expect((await connect(tv, eventsProtocols(token))).answer.status).toBe(401);
  });

  it("keep two sockets per TV, closing the oldest as replaced", async () => {
    const { tv, token } = await signedInTv();
    const first = await connect(tv, eventsProtocols(token));
    const second = await connect(tv, eventsProtocols(token));
    const third = await connect(tv, eventsProtocols(token));
    await vi.waitFor(() => expect(first.closes).toEqual([EVENTS_CLOSE.replaced]));
    expect(second.closes).toEqual([]);
    expect(third.closes).toEqual([]);
  });
});

// --- Changing a TV's profiles, and removing TVs (Phase 6) ---

/** Two TVs: the first with Living room and Kids, the second with Kids only. */
const twoTvs = async () => {
  const first = await signedInTv();
  const tv = await newTv("Kids TV");
  expect((await signInTv(first.browser, tv, [first.ids[1]])).status).toBe(200);
  const { trust, token } = await pickUp(tv);
  const view = openPull(trust, await (await pull(tv, token)).json());
  return { ...first, second: { tv, token, trust, view } };
};

describe("the TVs page", () => {
  it("lists each TV with its profiles, and forms to change or remove it", async () => {
    const { browser, trust } = await signedInTv();
    const page = await request(browser.visitor, "/devices");
    expect(page.status).toBe(200);
    const html = await page.text();
    expect(html).toContain(`<span data-tv-name="${trust.device}">…</span>`);
    expect(html).toContain(`data-velio="device-profiles" data-device-id="${trust.device}"`);
    expect(html).toContain('data-velio="device-remove"');
  });

  it("takes a profile from a TV: it can't read the profile's new versions; the other TV carries on", async () => {
    const { browser, ids, tv, token, trust, view, second } = await twoTvs();
    const socket = await connect(tv, eventsProtocols(token));
    const secondSocket = await connect(second.tv, eventsProtocols(second.token));
    await vi.waitFor(() => expect(socket.revs()).toHaveLength(1));
    const [start = 0] = socket.revs();

    await save(browser, state => withDeviceProfiles(state, trust.device, [ids[0]]), "/devices");
    await save(browser, state => {
      const kidsList = state.lists.find(item => item.record.name === "Kids list");
      return withList(state, list("Kids list, renamed"), [ids[1]], kidsList?.record.id).state;
    });
    await vi.waitFor(() => expect(socket.revs()).toEqual([start, start + 1, start + 2]));
    await vi.waitFor(() => expect(secondSocket.revs()).toHaveLength(3));

    // The TV that lost Kids: Living room only, and the old pull's keys open nothing new.
    const raw = await (await pull(tv, token)).json();
    const after = openPull(trust, raw, view);
    expect(after.profiles.map(profile => profile.name)).toEqual(["Living room"]);
    expect(names(after)).toEqual(["Living room list", "Shared list"]);
    expect(JSON.stringify(raw)).not.toContain(ids[1]);

    // The other TV has the renamed list.
    const secondAfter = openPull(
      second.trust,
      await (await pull(second.tv, second.token)).json(),
      second.view,
    );
    expect(names(secondAfter)).toEqual(["Kids list, renamed", "Shared list"]);
    socket.ws?.close(1000, "done");
    secondSocket.ws?.close(1000, "done");
  });

  it("removes a TV: its token stops working, its socket closes as removed, and the other TV carries on", async () => {
    const { browser, tv, token, trust, second } = await twoTvs();
    const socket = await connect(tv, eventsProtocols(token));
    await vi.waitFor(() => expect(socket.revs()).toHaveLength(1));

    await save(browser, state => withoutDevice(state, trust.device), "/devices");
    await vi.waitFor(() => expect(socket.closes).toEqual([EVENTS_CLOSE.removed]));
    expect(socket.messages).toContain("removed");
    expect((await pull(tv, token)).status).toBe(401);
    expect((await connect(tv, eventsProtocols(token))).answer.status).toBe(401);
    const row = await env.DB.prepare("SELECT id FROM devices WHERE id = ?").bind(trust.device).first();
    expect(row).toBeNull();

    const { state } = await openPage(browser, "/devices");
    expect(state.devices.map(device => device.name)).toEqual(["Kids TV"]);
    const secondAfter = openPull(
      second.trust,
      await (await pull(second.tv, second.token)).json(),
      second.view,
    );
    expect(secondAfter.profiles.map(profile => profile.name)).toEqual(["Kids"]);
  });
});
