import { describe, expect, it } from "vitest";
import {
  applyChange,
  buildChange,
  openState,
  readStoredState,
  withProfile,
  type OpenedState,
  type StoredState,
} from "../src/changes.ts";
import { newOwnerKeys } from "../src/owner.ts";
import {
  PROFILE_COLORS,
  PROFILE_PIN_LENGTH,
  profileColor,
  profileInitial,
  profilePinMatches,
  profileProblem,
  readProfile,
} from "../src/records.ts";

const ACCOUNT = "account-1";

const NOTHING: StoredState = { manifest: null, profiles: [], lists: [], devices: [] };
const OWNER = newOwnerKeys();
const empty = (): OpenedState => openState(OWNER, ACCOUNT, NOTHING);

const profileOf = (state: OpenedState, profileId: string) => {
  const found = state.profiles.find(profile => profile.record.id === profileId);
  if (!found) throw new Error("no such profile");
  return found.record;
};

describe("profile pictures", () => {
  it("give each new profile the first colour no other profile has", () => {
    let state = empty();
    const colors: number[] = [];
    for (let i = 0; i < PROFILE_COLORS.length; i++) {
      const made = withProfile(state, { name: `P${i}` });
      state = made.state;
      colors.push(profileOf(state, made.profileId).color ?? -1);
    }
    expect(colors).toEqual(PROFILE_COLORS.map((_color, index) => index));
  });

  it("keep a chosen colour, and keep it again when only the name changes", () => {
    const made = withProfile(empty(), { name: "Kids", color: 5 });
    expect(profileOf(made.state, made.profileId).color).toBe(5);
    const renamed = withProfile(made.state, { name: "Children" }, made.profileId);
    expect(profileOf(renamed.state, made.profileId)).toMatchObject({ name: "Children", color: 5, rev: 2 });
  });

  it("give a profile made before pictures a steady colour", () => {
    const old = { id: "abc123", color: undefined };
    expect(profileColor(old)).toBe(profileColor({ id: "abc123" }));
    expect(profileColor(old)).toBeGreaterThanOrEqual(0);
    expect(profileColor(old)).toBeLessThan(PROFILE_COLORS.length);
    expect(profileColor({ id: "abc123", color: 2 })).toBe(2);
  });

  it("draw the name's first letter in capitals, whole even outside the BMP", () => {
    expect(profileInitial("living room")).toBe("L");
    expect(profileInitial("  ömer")).toBe("Ö");
    expect(profileInitial("😀 party")).toBe("😀");
    expect(profileInitial("")).toBe("?");
  });

  it("are colours a white letter reads well on", () => {
    expect(PROFILE_COLORS).toHaveLength(8);
    for (const color of PROFILE_COLORS) expect(color.background).toMatch(/^#[0-9a-f]{6}$/);
  });
});

describe("profile PINs", () => {
  it("are set, kept, and removed", () => {
    const made = withProfile(empty(), { name: "Kids", pin: "1234" });
    expect(profileOf(made.state, made.profileId).pin).toBe("1234");
    const renamed = withProfile(made.state, { name: "Children" }, made.profileId);
    expect(profileOf(renamed.state, made.profileId).pin).toBe("1234");
    const removed = withProfile(renamed.state, { name: "Children", pin: null }, made.profileId);
    expect(profileOf(removed.state, made.profileId)).not.toHaveProperty("pin");
  });

  it("are four digits", () => {
    expect(PROFILE_PIN_LENGTH).toBe(4);
    expect(profileProblem({ name: "Kids", pin: "0042" })).toBeUndefined();
    for (const pin of ["123", "12345", "12a4", " 123", "１２３４"])
      expect(profileProblem({ name: "Kids", pin })).toBe("pin");
    expect(() => withProfile(empty(), { name: "Kids", pin: "12" })).toThrow(/4 digits/);
  });

  it("open the profile when typed right, or when there is none", () => {
    expect(profilePinMatches({ pin: "1234" }, "1234")).toBe(true);
    expect(profilePinMatches({ pin: "1234" }, "4321")).toBe(false);
    expect(profilePinMatches({}, "")).toBe(true);
  });
});

describe("profile drafts and records", () => {
  it("need a name that fits, and a colour from the list", () => {
    expect(profileProblem({ name: "  " })).toBe("name");
    expect(profileProblem({ name: "x".repeat(101) })).toBe("tooLong");
    expect(profileProblem({ name: "Kids", color: PROFILE_COLORS.length })).toBe("color");
    expect(profileProblem({ name: "Kids", color: 1.5 })).toBe("color");
    expect(profileProblem({ name: "Kids", color: 0 })).toBeUndefined();
  });

  it("read records with or without a colour and PIN, and refuse bad ones", () => {
    const base = { format: 1, id: "p1", rev: 1, name: "Kids" };
    expect(readProfile(base)).toEqual(base);
    expect(readProfile({ ...base, color: 3, pin: "0000" })).toEqual({ ...base, color: 3, pin: "0000" });
    expect(() => readProfile({ ...base, color: 9 })).toThrow(/color/);
    expect(() => readProfile({ ...base, pin: 1234 })).toThrow(/PIN/);
    expect(() => readProfile({ ...base, pin: "12345" })).toThrow(/PIN/);
  });

  it("go through the server and back sealed, with the colour and PIN", () => {
    const made = withProfile(empty(), { name: "Kids", color: 6, pin: "2468" });
    const change = buildChange(OWNER, empty(), made.state);
    // The server sees the record only sealed.
    expect(JSON.stringify(change)).not.toContain("2468");
    expect(JSON.stringify(change)).not.toContain("Kids");
    const { state: stored } = applyChange(
      NOTHING,
      JSON.parse(JSON.stringify(change)),
      ACCOUNT,
      OWNER.signing.publicKey,
    );
    const opened = openState(OWNER, ACCOUNT, readStoredState(JSON.parse(JSON.stringify(stored))));
    expect(profileOf(opened, made.profileId)).toMatchObject({ name: "Kids", color: 6, pin: "2468" });
  });
});
