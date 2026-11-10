// The owner's own copy of their lists (Phase 7): a plain JSON file made in
// the browser from the opened state, so the lists aren't locked in Velio. It
// isn't encrypted: it holds the list addresses, Xtream passwords and profile
// PINs as typed.

import type { OpenedState } from "./changes.ts";

export const EXPORT_FORMAT = 1;

export interface ExportedProfile {
  name: string;
  color?: number;
  pin?: string;
}

export interface ExportedList {
  id: string;
  name: string;
  kind: string;
  url?: string;
  server?: string;
  username?: string;
  password?: string;
  guideUrls?: string[];
  refreshHours: number;
  disabled?: boolean;
  /** The names of the profiles that use it. */
  profiles: string[];
}

export interface ListsExport {
  velio: "lists";
  format: typeof EXPORT_FORMAT;
  exportedAt: string;
  profiles: ExportedProfile[];
  lists: ExportedList[];
}

/** The account's profiles and lists, in the order the manifest gives them. */
export const exportLists = (state: OpenedState, now = new Date()): ListsExport => {
  const names = new Map(state.profiles.map(profile => [profile.record.id, profile.record.name]));
  return {
    velio: "lists",
    format: EXPORT_FORMAT,
    exportedAt: now.toISOString(),
    profiles: state.profiles.map(({ record: { name, color, pin } }) => ({ name, color, pin })),
    lists: state.lists.map(({ record, profiles }) => {
      const { format: _format, rev: _rev, ...fields } = record;
      return { ...fields, profiles: profiles.flatMap(id => names.get(id) ?? []) };
    }),
  };
};

/** The file name for an export made on `now`'s date, such as velio-lists-2026-11-10.json. */
export const exportFileName = (now = new Date()) => `velio-lists-${now.toISOString().slice(0, 10)}.json`;
