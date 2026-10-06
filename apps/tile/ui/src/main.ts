// Entry point. Every window loads this one page; the query string picks the
// screen, and only that screen's module boots.

import { bootAbout } from "./about";
import { bootWelcome } from "./onboarding";
import { bootSettings } from "./settings";
import { bootUpdates } from "./updates";

const isAboutScreen = new URLSearchParams(window.location.search).has("about");
const isWelcomeScreen = new URLSearchParams(window.location.search).has(
  "welcome",
);
const isUpdateScreen = new URLSearchParams(window.location.search).has(
  "updates",
);

async function boot(): Promise<void> {
  if (isWelcomeScreen) {
    await bootWelcome();
    return;
  }

  if (isAboutScreen) {
    await bootAbout();
    return;
  }
  if (isUpdateScreen) {
    await bootUpdates();
    return;
  }

  await bootSettings();
}

void boot();
