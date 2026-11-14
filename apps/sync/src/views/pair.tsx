// Signing in a TV (PLAN.md, "Signing in a TV"). The QR code opens
// /pair#<id>.<secret>; vault.js keeps the part after # (which never reaches
// the server), finds the TV with the pairing id and a proof of the secret,
// opens its sealed name and key, and sends the signed change that adds it.

import { Name, ProfilePic, TvName, VaultPage, type VaultPageProps } from "./vault.tsx";

export const PAIR_TITLE = "Sign in a TV";

/** The pairing page: type the TV's code, or come from its QR code with the code already filled in. */
/** `signedIn` is the TV just signed in, whose name vault.js fills in. */
export const PairPage = (props: VaultPageProps & { signedIn?: string }) => (
  <VaultPage {...props} title={PAIR_TITLE}>
    {props.signedIn !== undefined ? (
      <p class="done" role="status">
        <strong>
          <TvName id={props.signedIn} />
        </strong>{" "}
        is signed in. It loads your lists in a few seconds.
      </p>
    ) : null}
    <p class="note">
      On the TV, open VelioTV's Settings, then Account, and choose Sign in with a phone. Scan the QR code it
      shows with this phone's camera, or type the code below the QR code.
    </p>
    <form hx-post="/pair/find" hx-target="#pair-step" hx-confirm="velio" data-velio="pair-find">
      <label>
        Code on the TV
        <input
          type="text"
          data-field="code"
          required
          maxlength={120}
          placeholder="K7QM-2XHF-9WRT"
          autocomplete="off"
          autocapitalize="characters"
          spellcheck={false}
        />
      </label>
      <p id="form-problem" class="problem" role="alert"></p>
      <button type="submit">Find the TV</button>
    </form>
    <div id="pair-step"></div>
    <p>
      <a href="/account">Your account</a>
    </p>
  </VaultPage>
);

/**
 * The TV that was found: its name (sealed, so vault.js shows it once it
 * checks out), and the profiles to give it, all ticked.
 */
export const PairConfirm = (props: {
  pairingId: string;
  deviceId: string;
  kind: string;
  /** The TV's details sealed under each secret, as JSON (sync-core SealedDeviceInfo). */
  sealedJson: string;
  profileIds: string[];
}) => (
  <section data-sealed-device={props.sealedJson} data-kind={props.kind}>
    <h2>
      Sign in <span data-device-name>this TV</span>?
    </h2>
    <p class="problem" role="alert" data-pair-open-problem></p>
    {props.profileIds.length === 0 ? (
      <p>
        A TV shows the lists of the profiles you give it, and this account has none yet.{" "}
        <a href="/profiles">Add a profile</a>, then scan the code again.
      </p>
    ) : (
      <form
        hx-post={`/pair/${props.pairingId}`}
        hx-target="#main"
        hx-confirm="velio"
        data-velio="pair"
        data-pairing-id={props.pairingId}
        data-device-id={props.deviceId}
        hidden
      >
        <fieldset>
          <legend>Profiles on this TV</legend>
          {props.profileIds.map(profileId => (
            <label class="check">
              <input type="checkbox" data-profile-choice={profileId} checked />
              <ProfilePic id={profileId} />
              <Name kind="profile" id={profileId} />
            </label>
          ))}
        </fieldset>
        <p class="note">
          Only sign in a TV you're in front of: check that its name and code match what the TV shows.
        </p>
        <p id="pair-problem" class="problem" role="alert"></p>
        <button type="submit">Sign in the TV</button>
      </form>
    )}
  </section>
);
