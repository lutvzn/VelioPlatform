// The pages for profiles and lists (PLAN.md, "Routes"; Phase 2). The server
// can't read names, addresses or logins, so it renders the shape it knows
// from the manifest (ids, which profile has which list) with empty slots,
// and vault.js fills them from the ciphertext in [data-vault-state].
//
// Every input that holds something secret has no name, so HTMX never sends
// it: forms are marked data-velio="…", and vault.js sends a sealed, signed
// change in its place.

import {
  DEFAULT_REFRESH_HOURS,
  MAX_NAME_LENGTH,
  MAX_URL_LENGTH,
  PROFILE_COLORS,
  PROFILE_PIN_LENGTH,
  REFRESH_CHOICES,
} from "@velio/sync-core";
import type { Manifest, StoredState } from "@velio/sync-core";
import type { Child } from "hono/jsx";
import type { OwnerRecord } from "../account.ts";
import { deviceStatus, type DeviceRow } from "../lib/vault.ts";

export interface VaultPageProps {
  accountId: string;
  email: string;
  /** The owner's public signing key, so vault.js can tell remembered keys from before a reset. */
  signPublicKey: string;
  stored: StoredState;
  manifest: Manifest;
  /** The TVs' names and dates, from D1. */
  devices: DeviceRow[];
}

const Problem = () => <p id="form-problem" class="problem" role="alert"></p>;

/** The ciphertext the page shows, for vault.js to open. */
const VaultState = (props: { accountId: string; signPublicKey: string; stored: StoredState }) => (
  <div
    hidden
    data-vault-state={JSON.stringify(props.stored)}
    data-account-id={props.accountId}
    data-sign-public-key={props.signPublicKey}
  ></div>
);

/** The owner bundle, for vault.js to open with the unlock key it kept from the password. */
export const OwnerBundle = ({ owner }: { owner: OwnerRecord }) => (
  <div
    hidden
    data-owner-bundle={JSON.stringify(owner.bundle)}
    data-sign-public-key={owner.signPublicKey}
    data-box-public-key={owner.boxPublicKey}
  ></div>
);

/** Shown until the owner's keys are open in this tab. */
export const UnlockForm = ({ email }: { email: string }) => (
  <section data-locked>
    <p data-vault-status>Your lists are locked. Enter your password to open them in this browser.</p>
    <form hx-post="/unlock" hx-target="#unlock-result" hx-confirm="velio" data-velio="unlock">
      <input type="hidden" name="email" value={email} />
      <input type="email" hidden value={email} autocomplete="username" />
      <label>
        Password
        <input type="password" data-password autocomplete="current-password" required />
      </label>
      <RememberChoice />
      <p id="unlock-problem" class="problem" role="alert"></p>
      <button type="submit">Unlock</button>
    </form>
    <div id="unlock-result" hidden></div>
  </section>
);

export const RememberChoice = () => (
  <label class="check">
    <input type="checkbox" data-remember />
    <span>
      Remember this browser, so it opens your lists without the password. Only on your own computer.
    </span>
  </label>
);

/** A page that needs the owner's keys: the unlock form, then the page once vault.js has opened it. */
export const VaultPage = (props: VaultPageProps & { title: string; children: Child }) => (
  <>
    <VaultState accountId={props.accountId} signPublicKey={props.signPublicKey} stored={props.stored} />
    <section>
      <h1>{props.title}</h1>
      <p class="problem" data-vault-problem hidden></p>
    </section>
    <UnlockForm email={props.email} />
    <div data-needs-keys hidden>
      {props.children}
    </div>
  </>
);

export const Name = ({ kind, id }: { kind: "profile" | "list"; id: string }) =>
  kind === "profile" ? <span data-profile-name={id}>…</span> : <span data-list-name={id}>…</span>;

/** A TV's name, which only the owner can open: vault.js fills it in from the manifest. */
export const TvName = ({ id }: { id: string }) => <span data-tv-name={id}>…</span>;

const plural = (count: number, one: string, many: string) => `${count} ${count === 1 ? one : many}`;

// --- Profiles ---

/** A profile's picture: vault.js draws the name's first letter on the profile's colour. */
export const ProfilePic = ({ id }: { id: string }) => (
  <span class="pic" data-profile-pic={id} aria-hidden="true"></span>
);

export const ProfilesPage = (props: VaultPageProps) => {
  const { manifest } = props;
  return (
    <VaultPage {...props} title="Profiles">
      <p class="note">
        A profile is a set of lists, such as one for each person at home. Each TV shows the profiles you give
        it.
      </p>
      <Problem />
      {manifest.profiles.length === 0 ? <p>No profiles yet. Add the first one.</p> : null}
      <ul class="rows">
        {manifest.profiles.map(profile => (
          <li>
            <div class="row-head">
              <ProfilePic id={profile.id} />
              <strong>
                <Name kind="profile" id={profile.id} />
              </strong>
              <span class="note">
                {plural(profile.lists.length, "list", "lists")}
                <span data-profile-pin-note={profile.id}></span>
              </span>
            </div>
            <details>
              <summary>Change</summary>
              <form
                hx-post="/changes"
                hx-target="#main"
                hx-confirm="velio"
                data-velio="profile-edit"
                data-profile-id={profile.id}
              >
                <input type="hidden" name="next" value="/profiles" />
                <label>
                  Name
                  <input
                    type="text"
                    data-field="name"
                    required
                    maxlength={MAX_NAME_LENGTH}
                    autocomplete="off"
                  />
                </label>
                <fieldset class="swatches">
                  <legend>Colour</legend>
                  {PROFILE_COLORS.map((color, index) => (
                    <label class="swatch" title={color.name}>
                      {/* Named only so the radios form a group; vault.js sends the change instead of the form. */}
                      <input
                        type="radio"
                        name={`color-${profile.id}`}
                        value={String(index)}
                        data-color-choice
                      />
                      <span data-swatch={String(index)} aria-hidden="true"></span>
                      <span class="sr-only">{color.name}</span>
                    </label>
                  ))}
                </fieldset>
                <label>
                  Profile PIN
                  <input
                    type="text"
                    data-field="pin"
                    inputmode="numeric"
                    pattern={`[0-9]{${PROFILE_PIN_LENGTH}}`}
                    maxlength={PROFILE_PIN_LENGTH}
                    autocomplete="off"
                    placeholder="None"
                  />
                </label>
                <p class="note">
                  {PROFILE_PIN_LENGTH} digits a TV asks for before it opens this profile, or empty for none.
                  Like VelioTV's parental PIN, it keeps others at a shared TV out of the profile; it doesn't
                  encrypt anything.
                </p>
                <p class="problem" role="alert"></p>
                <button type="submit">Save</button>
              </form>
              <form
                hx-post="/changes"
                hx-target="#main"
                hx-confirm="velio"
                data-velio="profile-delete"
                data-profile-id={profile.id}
              >
                <input type="hidden" name="next" value="/profiles" />
                <p class="note">
                  Deleting a profile keeps its lists. TVs stop showing it, and its lists get new keys.
                </p>
                <button type="submit" class="danger">
                  Delete this profile
                </button>
              </form>
            </details>
          </li>
        ))}
      </ul>
      <form hx-post="/changes" hx-target="#main" hx-confirm="velio" data-velio="profile-add">
        <input type="hidden" name="next" value="/profiles" />
        <label>
          New profile
          <input
            type="text"
            data-field="name"
            required
            maxlength={MAX_NAME_LENGTH}
            placeholder="Living room, Sam, Kids…"
            autocomplete="off"
          />
        </label>
        <button type="submit">Add the profile</button>
      </form>
      <p>
        <a href="/lists">Your lists</a>
      </p>
    </VaultPage>
  );
};

// --- Lists ---

export const ListsPage = (props: VaultPageProps) => {
  const { manifest } = props;
  const holders = (listId: string) => manifest.profiles.filter(profile => profile.lists.includes(listId));
  return (
    <VaultPage {...props} title="Lists">
      <p class="note">
        The M3U playlists and Xtream logins of your own subscriptions, and the Stremio add-ons whose films and
        series your TVs show. Give each list to the profiles that should have it.
      </p>
      {manifest.lists.length === 0 ? <p>No lists yet.</p> : null}
      <ul class="rows">
        {manifest.lists.map(list => {
          const profiles = holders(list.id);
          return (
            <li>
              <div class="row-head">
                <a href={`/lists/${list.id}`}>
                  <strong>
                    <Name kind="list" id={list.id} />
                  </strong>
                </a>
                <span class="note" data-list-summary={list.id}></span>
              </div>
              <p class="note">
                {profiles.length === 0
                  ? "No profile has it yet."
                  : profiles.map((profile, index) => (
                      <>
                        {index > 0 ? ", " : "For "}
                        <Name kind="profile" id={profile.id} />
                      </>
                    ))}
              </p>
            </li>
          );
        })}
      </ul>
      <p>
        <a href="/lists/new" class="button">
          Add a list
        </a>
      </p>
      {manifest.profiles.length === 0 ? (
        <p class="note">
          A list reaches a TV through a profile. <a href="/profiles">Add a profile</a> to give lists to.
        </p>
      ) : (
        <p>
          <a href="/profiles">Your profiles</a>
        </p>
      )}
      {manifest.lists.length === 0 ? null : (
        <details>
          <summary>Download a copy</summary>
          <p class="note">
            A file with every list's address and login, and your profiles with their PINs, to keep or to use
            elsewhere. This browser makes it; it isn't encrypted, so keep it somewhere safe.
          </p>
          <button type="button" data-export>
            Download your lists
          </button>
        </details>
      )}
    </VaultPage>
  );
};

const RefreshChoice = () => (
  <label>
    Refresh
    <select data-field="refreshHours">
      {REFRESH_CHOICES.map(hours => (
        <option value={String(hours)} selected={hours === DEFAULT_REFRESH_HOURS}>
          {hours < 24
            ? `Every ${hours} hours`
            : hours === 24
              ? "Every day"
              : hours === 168
                ? "Every week"
                : `Every ${hours / 24} days`}
        </option>
      ))}
    </select>
  </label>
);

const TextField = (props: {
  field: string;
  label: string;
  type?: string;
  placeholder?: string;
  maxlength?: number;
}) => (
  <label>
    {props.label}
    <input
      type={props.type ?? "text"}
      data-field={props.field}
      maxlength={props.maxlength ?? MAX_URL_LENGTH}
      placeholder={props.placeholder}
      autocomplete="off"
      spellcheck={false}
    />
  </label>
);

/** Adding a list (no listId) or changing one; vault.js fills the fields from the list. */
export const ListPage = (props: VaultPageProps & { listId?: string }) => {
  const { manifest, listId } = props;
  const holders = new Set(
    listId
      ? manifest.profiles.filter(profile => profile.lists.includes(listId)).map(profile => profile.id)
      : [],
  );
  return (
    <VaultPage {...props} title={listId ? "Change the list" : "Add a list"}>
      <form
        hx-post="/changes"
        hx-target="#main"
        hx-confirm="velio"
        data-velio="list-save"
        data-list-id={listId}
        data-list-form
      >
        <input type="hidden" name="next" value="/lists" />
        <label>
          Kind
          <select data-field="kind">
            <option value="m3u">M3U playlist</option>
            <option value="xtream">Xtream login</option>
            <option value="stremio">Stremio add-on</option>
          </select>
        </label>
        <fieldset data-kind-only="m3u">
          <TextField field="url" label="Playlist address" type="url" placeholder="https://…" />
        </fieldset>
        <fieldset data-kind-only="stremio" hidden>
          <TextField
            field="addonUrl"
            label="Add-on address"
            type="url"
            placeholder="https://…/manifest.json or stremio://…"
          />
          <p class="note">
            Its catalogs show in Movies and Series on the TV, and its streams and subtitles on a title's page.
            Only streams with a web address play; torrents don't.
          </p>
        </fieldset>
        <fieldset data-kind-only="xtream" hidden>
          <TextField field="server" label="Server" placeholder="https://…" />
          <TextField field="username" label="Username" />
          <label>
            Password
            <input type="password" data-field="password" maxlength={MAX_URL_LENGTH} autocomplete="off" />
          </label>
        </fieldset>
        <TextField
          field="name"
          label="Name (optional)"
          maxlength={MAX_NAME_LENGTH}
          placeholder="The server's name"
        />
        <fieldset data-kind-not="stremio">
          <label>
            Guide addresses (optional)
            <textarea
              data-field="guideUrls"
              rows={2}
              spellcheck={false}
              placeholder="XMLTV addresses, one per line"
            ></textarea>
          </label>
          <RefreshChoice />
        </fieldset>
        <fieldset>
          <legend>Profiles that have it</legend>
          {manifest.profiles.length === 0 ? (
            <p class="note">
              No profiles yet. You can save the list now and <a href="/profiles">add a profile</a> after.
            </p>
          ) : (
            manifest.profiles.map(profile => (
              <label class="check">
                <input type="checkbox" data-profile-choice={profile.id} checked={holders.has(profile.id)} />
                <Name kind="profile" id={profile.id} />
              </label>
            ))
          )}
          {listId ? (
            <p class="note">
              Taking the list from a profile gives it a new key, so TVs with only that profile can't open it
              again.
            </p>
          ) : null}
        </fieldset>
        <label class="check">
          <input type="checkbox" data-field="disabled" />
          <span>Turned off: TVs keep it but don't load it</span>
        </label>
        <Problem />
        <button type="submit">{listId ? "Save the list" : "Add the list"}</button>
      </form>
      {listId ? (
        <form
          hx-post="/changes"
          hx-target="#main"
          hx-confirm="velio"
          data-velio="list-delete"
          data-list-id={listId}
        >
          <input type="hidden" name="next" value="/lists" />
          <button type="submit" class="danger">
            Delete this list
          </button>
        </form>
      ) : null}
      <p>
        <a href="/lists">Back to your lists</a>
      </p>
    </VaultPage>
  );
};

// --- TVs (Phase 6) ---

/**
 * The TVs signed in to the account: the profiles each has, changing them,
 * and removing a TV. Both give the profiles a TV loses new keys (vault.js,
 * with sync-core's withDeviceProfiles() and withoutDevice()).
 */
export const DevicesPage = (props: VaultPageProps) => {
  const { manifest } = props;
  return (
    <VaultPage {...props} title="TVs">
      <p class="note">
        Each TV shows the lists of the profiles you gave it. Taking a profile from a TV, or removing the TV,
        gives that profile and its lists new keys, so the TV can't open what changes after.
      </p>
      <Problem />
      {props.devices.length === 0 ? <p>No TVs yet.</p> : null}
      <ul class="rows">
        {props.devices.map(device => {
          const entry = manifest.devices.find(item => item.id === device.id);
          return (
            <li>
              <div class="row-head">
                <strong data-device-row={device.id}>
                  <TvName id={device.id} />
                </strong>
                <span class="note">{deviceStatus(device)}</span>
              </div>
              {entry ? (
                <>
                  <p class="note">
                    {entry.profiles.map((profileId, index) => (
                      <>
                        {index > 0 ? ", " : ""}
                        <Name kind="profile" id={profileId} />
                      </>
                    ))}
                  </p>
                  <details>
                    <summary>{device.signed_out_at === null ? "Change" : "Remove"}</summary>
                    {device.signed_out_at === null ? (
                      <form
                        hx-post="/changes"
                        hx-target="#main"
                        hx-confirm="velio"
                        data-velio="device-profiles"
                        data-device-id={device.id}
                      >
                        <input type="hidden" name="next" value="/devices" />
                        <fieldset>
                          <legend>Profiles on this TV</legend>
                          {manifest.profiles.map(profile => (
                            <label class="check">
                              <input
                                type="checkbox"
                                data-profile-choice={profile.id}
                                checked={entry.profiles.includes(profile.id)}
                              />
                              <ProfilePic id={profile.id} />
                              <Name kind="profile" id={profile.id} />
                            </label>
                          ))}
                        </fieldset>
                        <p class="problem" role="alert"></p>
                        <button type="submit">Save</button>
                      </form>
                    ) : null}
                    <form
                      hx-post="/changes"
                      hx-target="#main"
                      hx-confirm="velio"
                      data-velio="device-remove"
                      data-device-id={device.id}
                    >
                      <input type="hidden" name="next" value="/devices" />
                      <p class="note">
                        The TV signs out the next time it syncs: within seconds when it's on. To use it again,
                        sign it in again.
                      </p>
                      <p class="problem" role="alert"></p>
                      <button type="submit" class="danger">
                        Remove this TV
                      </button>
                    </form>
                  </details>
                </>
              ) : null}
            </li>
          );
        })}
      </ul>
      <p>
        <a href="/pair" class="button">
          Sign in a TV
        </a>
      </p>
    </VaultPage>
  );
};
