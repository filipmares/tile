// The one in-app confirmation dialog. Restore defaults and the shortcut
// recorder both ask through it, so every "are you sure" looks and behaves the
// same: focus moves into the dialog, Esc cancels, the heading labels it, and
// focus goes back to whatever opened it when it closes.

export interface ConfirmOptions {
  /** The question, shown as the dialog's heading and used as its name. */
  title: string;
  /** What happens if the user confirms. */
  message: string;
  confirmLabel: string;
  cancelLabel?: string;
  /** Styles the confirm button as destructive. */
  danger?: boolean;
  /**
   * Where focus goes when the dialog closes. A function, because the element
   * that opened the dialog may have been re-rendered while it was open.
   */
  returnFocus: () => HTMLElement | null;
}

let sequence = 0;

/** Asks `options.title`; resolves `true` for confirm, `false` for Cancel or Esc. */
export function confirmDialog(options: ConfirmOptions): Promise<boolean> {
  const id = `confirm-dialog-${++sequence}`;
  const dialog = document.createElement("dialog");
  dialog.className = "confirm-dialog";
  dialog.setAttribute("aria-labelledby", `${id}-title`);
  dialog.setAttribute("aria-describedby", `${id}-message`);

  const title = document.createElement("h2");
  title.id = `${id}-title`;
  title.className = "confirm-dialog__title";
  title.textContent = options.title;

  const message = document.createElement("p");
  message.id = `${id}-message`;
  message.className = "panel__hint";
  message.textContent = options.message;

  const confirm = document.createElement("button");
  confirm.type = "button";
  confirm.className = options.danger ? "button button--danger" : "button";
  confirm.textContent = options.confirmLabel;

  const cancel = document.createElement("button");
  cancel.type = "button";
  cancel.className = "button";
  cancel.textContent = options.cancelLabel ?? "Cancel";

  const actions = document.createElement("div");
  actions.className = "panel__actions";
  actions.append(confirm, cancel);
  dialog.append(title, message, actions);

  return new Promise((resolve) => {
    let confirmed = false;
    confirm.addEventListener("click", () => {
      confirmed = true;
      dialog.close();
    });
    cancel.addEventListener("click", () => dialog.close());
    // Esc fires `cancel` and then `close`, so it needs no handler of its own.
    dialog.addEventListener("close", () => {
      dialog.remove();
      options.returnFocus()?.focus();
      resolve(confirmed);
    });
    document.body.append(dialog);
    dialog.showModal();
    confirm.focus();
  });
}
