import { useEffect, type RefObject } from "react";

const FOCUSABLE = [
  "a[href]",
  "button:not([disabled])",
  "input:not([disabled]):not([type='hidden'])",
  "select:not([disabled])",
  "textarea:not([disabled])",
  "summary",
  "[tabindex]:not([tabindex='-1'])",
].join(",");

export function focusableWithin(root: HTMLElement): HTMLElement[] {
  return Array.from(root.querySelectorAll<HTMLElement>(FOCUSABLE)).filter(
    (element) => !element.closest("[inert]") && element.getAttribute("aria-hidden") !== "true",
  );
}

/**
 * Modal focus handling for the settings sheet and the setup guide: focus
 * moves into the dialog when it opens, Tab and Shift+Tab cycle inside it,
 * and focus goes back to whatever opened it when it closes. Escape is left
 * to App, which closes the topmost layer.
 *
 * `initialFocus` picks the element focused on open; without it the first
 * focusable control is used, or the dialog itself.
 */
export function useDialogFocus(
  dialogRef: RefObject<HTMLElement | null>,
  initialFocus?: RefObject<HTMLElement | null>,
): void {
  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;
    const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    if (!dialog.contains(document.activeElement)) {
      const target = initialFocus?.current ?? focusableWithin(dialog)[0] ?? dialog;
      target.focus();
    }

    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Tab") return;
      const items = focusableWithin(dialog);
      if (items.length === 0) {
        event.preventDefault();
        dialog.focus();
        return;
      }
      const first = items[0];
      const last = items[items.length - 1];
      const active = document.activeElement;
      if (event.shiftKey && (active === first || active === dialog || !dialog.contains(active))) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && (active === last || !dialog.contains(active))) {
        event.preventDefault();
        first.focus();
      }
    };
    // A control that disappears under focus (a step's button swapped out, a
    // field hidden by a toggle) drops focus to <body>, outside the dialog;
    // pull it back in so the keyboard never ends up behind the modal.
    const onFocusOut = (event: FocusEvent) => {
      const next = event.relatedTarget;
      if (next instanceof Node && dialog.contains(next)) return;
      if (next instanceof Node) {
        // Focus moved somewhere real outside the dialog (another window
        // layer, a click on the scrim): leave it alone.
        return;
      }
      window.setTimeout(() => {
        if (!dialog.isConnected) return;
        const active = document.activeElement;
        if (active === null || active === document.body) {
          (focusableWithin(dialog)[0] ?? dialog).focus();
        }
      }, 0);
    };
    dialog.addEventListener("keydown", onKeyDown);
    dialog.addEventListener("focusout", onFocusOut);

    return () => {
      dialog.removeEventListener("keydown", onKeyDown);
      dialog.removeEventListener("focusout", onFocusOut);
      // Restore only when focus would otherwise be lost: if the close already
      // moved focus somewhere deliberate (the next dialog), keep it there.
      const active = document.activeElement;
      const lost = active === null || active === document.body || dialog.contains(active);
      if (lost && opener?.isConnected) opener.focus();
    };
  }, [dialogRef, initialFocus]);
}
