// Path routing for the SPA: one URL per screen, so a reload keeps the screen
// and back/forward work. Hand-rolled because the routes are flat with no
// params. The server answers every non-/api, non-/assets path with
// index.html (spa_index), so any path here is safe to reload.
//
// View state (the selected month, open panels) stays in app memory, not the URL.
import { useEffect, useState } from "react";

export type Screen = "ledger" | "flow" | "balances" | "settings";

export const SCREENS: { screen: Screen; path: string; label: string }[] = [
  { screen: "ledger", path: "/ledger", label: "Ledger" },
  { screen: "flow", path: "/flow", label: "Flow" },
  { screen: "balances", path: "/balances", label: "Balances" },
  { screen: "settings", path: "/settings", label: "Settings" },
];

const DEFAULT: Screen = "ledger";

// Plaid sends OAuth banks back here. It shows Settings, and the URL is left
// alone until Connections has read it (Link needs the full return URL).
export const OAUTH_RETURN = "/oauth-return";

export function isOauthReturn(): boolean {
  return window.location.pathname === OAUTH_RETURN;
}

export function pathOf(screen: Screen): string {
  return SCREENS.find((s) => s.screen === screen)!.path;
}

function screenAt(pathname: string): Screen | null {
  if (pathname === OAUTH_RETURN) return "settings";
  return SCREENS.find((s) => s.path === pathname)?.screen ?? null;
}

// `/` and unknown paths become the default screen without adding a history
// entry.
function resolve(): Screen {
  const s = screenAt(window.location.pathname);
  if (s) return s;
  window.history.replaceState(null, "", pathOf(DEFAULT));
  return DEFAULT;
}

const NAV_EVENT = "outflow:navigate";

export function navigate(path: string, opts: { replace?: boolean } = {}) {
  if (path === window.location.pathname) return;
  if (opts.replace) window.history.replaceState(null, "", path);
  else {
    window.history.pushState(null, "", path);
    window.scrollTo(0, 0);
  }
  window.dispatchEvent(new Event(NAV_EVENT));
}

export function useScreen(): Screen {
  const [screen, setScreen] = useState<Screen>(resolve);
  useEffect(() => {
    const sync = () => setScreen(resolve());
    window.addEventListener("popstate", sync);
    window.addEventListener(NAV_EVENT, sync);
    return () => {
      window.removeEventListener("popstate", sync);
      window.removeEventListener(NAV_EVENT, sync);
    };
  }, []);
  return screen;
}

// For an <a href> nav link: a plain left click routes in-app; modified or
// middle clicks fall through so the browser can open a new tab.
export function onLinkClick(e: React.MouseEvent<HTMLAnchorElement>, path: string) {
  if (e.defaultPrevented || e.button !== 0) return;
  if (e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;
  e.preventDefault();
  navigate(path);
}
