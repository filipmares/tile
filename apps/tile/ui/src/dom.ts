// Element lookups shared by every screen. All screens live in the one
// index.html, so every lookup resolves on every screen.

/** Non-null `querySelector`, throwing at boot if the markup is wrong. */
function el<T extends HTMLElement>(selector: string): T {
  const node = document.querySelector<T>(selector);
  if (!node) throw new Error(`missing element: ${selector}`);
  return node;
}

export const dom = {
  app: el<HTMLElement>("#app"),
  about: el<HTMLElement>("#about"),
  aboutVersion: el<HTMLParagraphElement>("#about-version"),
  aboutCheckUpdate: el<HTMLButtonElement>("#about-check-update"),
  aboutUpdateStatus: el<HTMLParagraphElement>("#about-update-status"),
  github: el<HTMLButtonElement>("#github"),
  bindings: el<HTMLUListElement>("#bindings"),
  assignedBindings: el<HTMLUListElement>("#assigned-bindings"),
  allShortcutsCount: el<HTMLSpanElement>("#all-shortcuts-count"),
  settingsSearch: el<HTMLInputElement>("#settings-search"),
  settingsSearchEmpty: el<HTMLParagraphElement>("#settings-search-empty"),
  settingsSearchStatus: el<HTMLParagraphElement>("#settings-search-status"),
  hotkeyHookWarning: el<HTMLParagraphElement>("#hotkey-hook-warning"),
  hotkeyApplyError: el<HTMLParagraphElement>("#hotkey-apply-error"),
  recordingStatus: el<HTMLParagraphElement>("#recording-status"),
  bindingError: el<HTMLParagraphElement>("#binding-error"),
  cyclingError: el<HTMLParagraphElement>("#cycling-error"),
  gapsError: el<HTMLParagraphElement>("#gaps-error"),
  motionError: el<HTMLParagraphElement>("#motion-error"),
  launchError: el<HTMLParagraphElement>("#launch-error"),
  advancedError: el<HTMLParagraphElement>("#advanced-error"),
  resetError: el<HTMLParagraphElement>("#reset-error"),
  gapWindow: el<HTMLInputElement>("#gap-window"),
  gapWindowNumber: el<HTMLInputElement>("#gap-window-number"),
  gapEdgeTop: el<HTMLInputElement>("#gap-edge-top"),
  gapEdgeBottom: el<HTMLInputElement>("#gap-edge-bottom"),
  gapEdgeLeft: el<HTMLInputElement>("#gap-edge-left"),
  gapEdgeRight: el<HTMLInputElement>("#gap-edge-right"),
  gapSkipTop: el<HTMLInputElement>("#gap-skip-top"),
  gapMainOnly: el<HTMLInputElement>("#gap-main-only"),
  subsequentMode: el<HTMLSelectElement>("#subsequent-mode"),
  cycleSizes: el<HTMLFieldSetElement>("#cycle-sizes"),
  cycleSizesGrid: el<HTMLDivElement>("#cycle-sizes-grid"),
  animate: el<HTMLInputElement>("#animate-moves"),
  animationDuration: el<HTMLInputElement>("#animation-duration"),
  animationDurationNumber: el<HTMLInputElement>("#animation-duration-number"),
  launch: el<HTMLInputElement>("#launch-on-login"),
  advancedAlmostMaximizeWidth: el<HTMLInputElement>(
    "#advanced-almost-maximize-width",
  ),
  advancedAlmostMaximizeHeight: el<HTMLInputElement>(
    "#advanced-almost-maximize-height",
  ),
  advancedSizeStep: el<HTMLInputElement>("#advanced-size-step"),
  advancedWidthStep: el<HTMLInputElement>("#advanced-width-step"),
  advancedMoveStep: el<HTMLInputElement>("#advanced-move-step"),
  advancedMinimumWidth: el<HTMLInputElement>("#advanced-minimum-width"),
  advancedMinimumHeight: el<HTMLInputElement>("#advanced-minimum-height"),
  advancedAnimationFps: el<HTMLInputElement>("#advanced-animation-fps"),
  reset: el<HTMLButtonElement>("#reset"),
  resetStatus: el<HTMLParagraphElement>("#reset-status"),
  undoReset: el<HTMLButtonElement>("#undo-reset"),
  showWelcome: el<HTMLButtonElement>("#show-welcome"),
  permissionPanel: el<HTMLElement>("#permission-panel"),
  welcome: el<HTMLElement>("#welcome"),
  welcomeHome: el<HTMLSpanElement>("#welcome-home"),
  welcomeStage: el<HTMLDivElement>("#welcome-stage"),
  welcomeScreens: el<HTMLDivElement>("#welcome-screens"),
  welcomeGhost: el<HTMLDivElement>("#welcome-ghost"),
  welcomePane: el<HTMLDivElement>("#welcome-pane"),
  welcomeTrack: el<HTMLDivElement>("#welcome-track"),
  welcomeEnd: el<HTMLDivElement>("#welcome-end"),
  welcomeEndLine: el<HTMLParagraphElement>("#welcome-end-line"),
  welcomeLede: el<HTMLParagraphElement>("#welcome-lede"),
  welcomeEndAside: el<HTMLParagraphElement>("#welcome-end-aside"),
  welcomeDots: el<HTMLDivElement>("#welcome-dots"),
  welcomeSkip: el<HTMLButtonElement>("#welcome-skip"),
  welcomeSkipKey: el<HTMLSpanElement>("#welcome-skip-key"),
  welcomeProgress: el<HTMLParagraphElement>("#welcome-progress"),
  welcomeNote: el<HTMLParagraphElement>("#welcome-note"),
  welcomeDismiss: el<HTMLButtonElement>("#welcome-dismiss"),
  welcomeLaunch: el<HTMLInputElement>("#welcome-launch-on-login"),
  welcomeLaunchError: el<HTMLParagraphElement>("#welcome-launch-error"),
  grant: el<HTMLButtonElement>("#grant-permission"),
  openAccessibility: el<HTMLButtonElement>("#open-accessibility"),
  developmentPanel: el<HTMLElement>("#development-panel"),
  developmentConfigDir: el<HTMLParagraphElement>("#development-config-dir"),
  recoveryPanel: el<HTMLElement>("#recovery-panel"),
  recoveryMessage: el<HTMLParagraphElement>("#recovery-message"),
  recoveryPath: el<HTMLParagraphElement>("#recovery-path"),
  recoveryOpenFolder: el<HTMLButtonElement>("#recovery-open-folder"),
  recoveryDismiss: el<HTMLButtonElement>("#recovery-dismiss"),
  recoveryStatus: el<HTMLParagraphElement>("#recovery-status"),
  launchDevelopmentNote: el<HTMLParagraphElement>("#launch-development-note"),
  updates: el<HTMLElement>("#updates"),
  updateVersion: el<HTMLParagraphElement>("#update-version"),
  updateStatus: el<HTMLParagraphElement>("#update-status"),
  updateNotes: el<HTMLParagraphElement>("#update-notes"),
  updateProgress: el<HTMLProgressElement>("#update-progress"),
  updateProgressDetail: el<HTMLParagraphElement>("#update-progress-detail"),
  checkUpdate: el<HTMLButtonElement>("#check-update"),
  installUpdate: el<HTMLButtonElement>("#install-update"),
  updateConfirmation: el<HTMLDialogElement>("#update-confirmation"),
  updateConfirmationMessage: el<HTMLParagraphElement>(
    "#update-confirmation-message",
  ),
  confirmUpdate: el<HTMLButtonElement>("#confirm-update"),
  cancelUpdate: el<HTMLButtonElement>("#cancel-update"),
};

/** Hides every screen except `screen`, which becomes the whole window. */
export function showOnly(screen: HTMLElement, modifier: string): void {
  dom.app.classList.add(modifier);
  for (const child of dom.app.children) {
    if (child !== screen) (child as HTMLElement).hidden = true;
  }
  screen.hidden = false;
}
