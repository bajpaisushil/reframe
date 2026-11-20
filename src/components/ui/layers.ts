// Overlay layer stack shared by Dialog, Menu and Popover.
//
// - Escape goes to the topmost layer only (a menu inside a dialog closes before the dialog).
// - While a modal layer is open, the app root is made `inert`, so the page behind it can neither be
//   focused nor clicked, and focus that escapes the modal is pulled back into it. Layers opened on
//   top of a modal (a popover inside a dialog) are portaled outside the root and stay usable.

export interface Layer {
  id: number;
  modal: boolean;
  /** Root element of the layer's content (for containment checks). */
  getElement: () => HTMLElement | null;
  /** Elements that belong to the layer without being inside it (e.g. a menu's trigger). */
  getRelatedElements?: () => Array<HTMLElement | null>;
  onEscape?: () => void;
}

const stack: Layer[] = [];
let nextId = 1;
let listenersInstalled = false;

export function createLayerId(): number {
  return nextId++;
}

function appRoot(): HTMLElement | null {
  return typeof document === "undefined" ? null : document.getElementById("root");
}

function updateInert(): void {
  const root = appRoot();
  if (!root) return;
  const modalOpen = stack.some((layer) => layer.modal);
  if (root.inert !== modalOpen) root.inert = modalOpen;
}

function topModalIndex(): number {
  for (let i = stack.length - 1; i >= 0; i--) if (stack[i].modal) return i;
  return -1;
}

/** Is `node` inside the layer at `index` or any layer stacked above it? */
function insideLayersFrom(index: number, node: Node): boolean {
  for (let i = index; i < stack.length; i++) {
    const layer = stack[i];
    if (layer.getElement()?.contains(node)) return true;
    if (layer.getRelatedElements?.().some((el) => el?.contains(node))) return true;
  }
  return false;
}

export function focusableElements(container: HTMLElement): HTMLElement[] {
  const selector = [
    "a[href]",
    "button:not([disabled])",
    "input:not([disabled]):not([type='hidden'])",
    "select:not([disabled])",
    "textarea:not([disabled])",
    "[tabindex]:not([tabindex='-1'])",
    "[contenteditable='true']",
  ].join(",");
  return Array.from(container.querySelectorAll<HTMLElement>(selector)).filter(
    (el) =>
      // `[tabindex]` also matches disabled controls; they cannot actually take focus.
      !(el as HTMLButtonElement).disabled &&
      !el.closest("[inert], fieldset[disabled]") &&
      el.getClientRects().length > 0,
  );
}

function onKeyDown(event: KeyboardEvent): void {
  if (event.key !== "Escape" || stack.length === 0 || event.defaultPrevented) return;
  const top = stack[stack.length - 1];
  if (!top.onEscape) return;
  event.preventDefault();
  event.stopPropagation();
  top.onEscape();
}

function onFocusIn(event: FocusEvent): void {
  const index = topModalIndex();
  if (index === -1 || !(event.target instanceof Node)) return;
  if (insideLayersFrom(index, event.target)) return;
  const modal = stack[index].getElement();
  if (!modal) return;
  const [first] = focusableElements(modal);
  (first ?? modal).focus({ preventScroll: true });
}

function installListeners(): void {
  if (listenersInstalled || typeof document === "undefined") return;
  listenersInstalled = true;
  // Capture phase: runs before component handlers, so the topmost layer always wins.
  document.addEventListener("keydown", onKeyDown, true);
  document.addEventListener("focusin", onFocusIn, true);
}

export function pushLayer(layer: Layer): void {
  installListeners();
  stack.push(layer);
  updateInert();
}

export function removeLayer(id: number): void {
  const index = stack.findIndex((layer) => layer.id === id);
  if (index !== -1) stack.splice(index, 1);
  updateInert();
}

export function isTopLayer(id: number): boolean {
  return stack.length > 0 && stack[stack.length - 1].id === id;
}

/** True when `node` is inside the given layer or a layer opened on top of it. */
export function isInsideLayerOrAbove(id: number, node: Node): boolean {
  const index = stack.findIndex((layer) => layer.id === id);
  return index !== -1 && insideLayersFrom(index, node);
}
