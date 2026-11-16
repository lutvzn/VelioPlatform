// site.js: the little the website does in the browser. Everything else is
// plain forms, sent by HTMX, and answered by the server.
//
// - Marks the tab of the page shown.
// - Shows a list form's fields for its kind (M3U or Xtream).
// - Colours profile pictures and swatches from data-color, through the
//   CSSOM, as the CSP keeps style attributes out.
// - Removes Turnstile's widgets a swap took out, and resets one after a
//   problem, so the next try gets a new token.

declare global {
  interface Window {
    turnstile?: { reset: () => void; remove: (container: Element) => void };
  }
}

const showKind = (form: HTMLFormElement) => {
  const kind = form.querySelector<HTMLSelectElement>('[name="kind"]')?.value ?? "";
  for (const fieldset of form.querySelectorAll<HTMLElement>("[data-kind-only]"))
    fieldset.hidden = fieldset.dataset.kindOnly !== kind;
};

const showKinds = () => {
  for (const form of document.querySelectorAll<HTMLFormElement>("form[data-list-form]")) showKind(form);
};

const HEX = /^#[0-9a-f]{6}$/i;
const paint = () => {
  for (const element of document.querySelectorAll<HTMLElement>("[data-color]")) {
    const color = element.dataset.color ?? "";
    if (HEX.test(color)) element.style.backgroundColor = color;
  }
};

let widgets: Element[] = [];
document.addEventListener("htmx:before:swap", () => {
  widgets = [...document.querySelectorAll(".cf-turnstile")];
});
const removeGoneWidgets = () => {
  for (const widget of widgets)
    if (!widget.isConnected)
      try {
        window.turnstile?.remove(widget);
      } catch {
        // Already gone.
      }
  widgets = [];
};

/** /tv-login and /link are under TVs. A boosted link's address changes
 * after its swap, so this runs again a moment later. */
const markNav = () => {
  const here = ["/tv-login", "/link"].includes(location.pathname) ? "/devices" : location.pathname;
  for (const link of document.querySelectorAll<HTMLAnchorElement>("#nav a")) {
    const path = new URL(link.href).pathname;
    if (here === path || here.startsWith(`${path}/`)) link.setAttribute("aria-current", "page");
    else link.removeAttribute("aria-current");
  }
};

document.addEventListener("htmx:after:swap", () => {
  removeGoneWidgets();
  showKinds();
  paint();
  markNav();
  setTimeout(markNav, 0);
});
window.addEventListener("popstate", () => setTimeout(markNav, 0));
document.addEventListener("htmx:response:error", () => {
  if (window.turnstile && document.querySelector(".cf-turnstile")) window.turnstile.reset();
});
document.addEventListener("change", event => {
  const target = event.target;
  if (
    target instanceof HTMLSelectElement &&
    target.name === "kind" &&
    target.form?.dataset.listForm !== undefined
  )
    showKind(target.form);
});

markNav();
showKinds();
paint();
