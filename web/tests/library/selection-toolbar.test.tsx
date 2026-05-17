/**
 * Multi-select M7: smoke-tests for `<SelectionToolbar>` rendering.
 * The vitest env is `node` (no DOM), so we render to a React element
 * tree and walk it instead of using @testing-library/react. We
 * confirm structural invariants the list pages rely on:
 *   - The Done button (X icon) wires `onDone`.
 *   - Primary actions render with their onClick + label.
 *   - Overflow actions render in BOTH the inline sm+ slot and the
 *     sm- dropdown (Tailwind responsive classes, not JS branching).
 *   - `isPending`, per-action `disabled`, and `count === 0` all
 *     disable action buttons so a mid-mutation toolbar can't
 *     double-fire.
 *   - `onSelectAll` is hidden when everything is already selected.
 *
 * Plan: `~/.claude/plans/multi-select-bulk-actions-1.0.md` (M7).
 */
import { describe, expect, it, vi } from "vitest";

import {
  SelectionToolbar,
  type SelectionAction,
} from "@/components/library/SelectionToolbar";

type AnyEl = { type: unknown; props: Record<string, unknown> };

/** Walks a React element tree (returned by calling the component
 *  function directly) and yields every element node, allowing tests
 *  to use a predicate over an iterable. */
function* walk(node: unknown): Generator<AnyEl> {
  if (!node || typeof node !== "object") return;
  // Arrays of children are common from `.map(...)`.
  if (Array.isArray(node)) {
    for (const c of node) yield* walk(c);
    return;
  }
  const el = node as AnyEl;
  if (typeof el.type !== "undefined") yield el;
  const children = (el.props as { children?: unknown } | undefined)?.children;
  if (children !== undefined) yield* walk(children);
}

/** Concatenates every string descendant of a node (its children's
 *  recursive text content). Used to locate the action button by its
 *  label without depending on the shadcn Button's internal layout. */
function textOf(node: unknown): string {
  if (node === null || node === undefined) return "";
  if (typeof node === "string" || typeof node === "number") return String(node);
  if (Array.isArray(node)) return node.map(textOf).join("");
  if (typeof node === "object") {
    const children = (node as { props?: { children?: unknown } }).props
      ?.children;
    return textOf(children);
  }
  return "";
}

/** Finds an element whose recursive text content contains `label`
 *  AND which has an `onClick` prop (i.e. the actual button, not the
 *  enclosing layout div). */
function findClickableByLabel(tree: unknown, label: string): AnyEl | undefined {
  for (const el of walk(tree)) {
    if (typeof el.props.onClick !== "function") continue;
    if (textOf(el).includes(label)) return el;
  }
  return undefined;
}

function action(
  id: string,
  overrides: Partial<SelectionAction> = {},
): SelectionAction {
  return {
    id,
    label: id,
    onClick: vi.fn(),
    ...overrides,
  };
}

describe("<SelectionToolbar>", () => {
  it("renders with aria-label reflecting the count (singular)", () => {
    const tree = SelectionToolbar({
      count: 1,
      total: 10,
      primary: [],
      onDone: () => {},
      onClear: () => {},
    });
    const root = tree as unknown as {
      props: { role: string; "aria-label": string };
    };
    expect(root.props.role).toBe("toolbar");
    expect(root.props["aria-label"]).toBe("1 item selected");
  });

  it("renders with aria-label reflecting the count (plural)", () => {
    const tree = SelectionToolbar({
      count: 3,
      total: 10,
      primary: [],
      onDone: () => {},
      onClear: () => {},
    });
    const root = tree as unknown as { props: { "aria-label": string } };
    expect(root.props["aria-label"]).toBe("3 items selected");
  });

  it("Done button wires onDone via aria-label", () => {
    const onDone = vi.fn();
    const tree = SelectionToolbar({
      count: 2,
      total: 10,
      primary: [],
      onDone,
      onClear: () => {},
    });
    let doneBtn: AnyEl | undefined;
    for (const el of walk(tree)) {
      if (el.props["aria-label"] === "Done — exit select mode") {
        doneBtn = el;
        break;
      }
    }
    expect(doneBtn).toBeDefined();
    expect(typeof doneBtn!.props.onClick).toBe("function");
    (doneBtn!.props.onClick as () => void)();
    expect(onDone).toHaveBeenCalledTimes(1);
  });

  it("primary action buttons invoke their onClick", () => {
    const a = action("mark-read");
    const tree = SelectionToolbar({
      count: 2,
      total: 10,
      primary: [a],
      onDone: () => {},
      onClear: () => {},
    });
    const btn = findClickableByLabel(tree, "mark-read");
    expect(btn).toBeDefined();
    (btn!.props.onClick as () => void)();
    expect(a.onClick).toHaveBeenCalledTimes(1);
  });

  it("disables primary actions when count === 0", () => {
    const a = action("mark-read");
    const tree = SelectionToolbar({
      count: 0,
      total: 10,
      primary: [a],
      onDone: () => {},
      onClear: () => {},
    });
    const btn = findClickableByLabel(tree, "mark-read");
    expect(btn!.props.disabled).toBe(true);
  });

  it("disables primary actions when isPending is true", () => {
    const a = action("mark-read");
    const tree = SelectionToolbar({
      count: 5,
      total: 10,
      primary: [a],
      onDone: () => {},
      onClear: () => {},
      isPending: true,
    });
    const btn = findClickableByLabel(tree, "mark-read");
    expect(btn!.props.disabled).toBe(true);
  });

  it("honors per-action disabled prop independently of isPending/count", () => {
    const a = action("mark-read", { disabled: true });
    const tree = SelectionToolbar({
      count: 5,
      total: 10,
      primary: [a],
      onDone: () => {},
      onClear: () => {},
      isPending: false,
    });
    const btn = findClickableByLabel(tree, "mark-read");
    expect(btn!.props.disabled).toBe(true);
  });

  it("renders overflow actions in both the sm+ inline slot and the sm- dropdown", () => {
    // Tailwind responsive variants (hidden sm:flex / flex sm:hidden)
    // mean both branches always render — visibility is CSS-driven.
    // The test verifies BOTH paths emit a clickable per overflow
    // entry so SSR / hydration sees consistent markup at every
    // breakpoint.
    const a = action("add-to-collection");
    const tree = SelectionToolbar({
      count: 2,
      total: 10,
      primary: [],
      overflow: [a],
      onDone: () => {},
      onClear: () => {},
    });
    let hits = 0;
    for (const el of walk(tree)) {
      if (typeof el.props.onClick !== "function") continue;
      if (textOf(el).includes("add-to-collection")) hits += 1;
    }
    // One clickable for the inline sm+ Button, one for the
    // DropdownMenuItem in the sm- dropdown.
    expect(hits).toBeGreaterThanOrEqual(2);
  });

  it("hides Select all when every item is already selected", () => {
    const tree = SelectionToolbar({
      count: 10,
      total: 10,
      primary: [],
      onDone: () => {},
      onClear: () => {},
      onSelectAll: () => {},
    });
    expect(findClickableByLabel(tree, "Select all")).toBeUndefined();
  });

  it("hides Clear when count is zero (nothing to clear)", () => {
    const tree = SelectionToolbar({
      count: 0,
      total: 10,
      primary: [],
      onDone: () => {},
      onClear: () => {},
    });
    expect(findClickableByLabel(tree, "Clear")).toBeUndefined();
  });

  it("destructive action uses the destructive variant", () => {
    const a = action("remove", { destructive: true });
    const tree = SelectionToolbar({
      count: 2,
      total: 10,
      primary: [a],
      onDone: () => {},
      onClear: () => {},
    });
    const btn = findClickableByLabel(tree, "remove");
    expect(btn!.props.variant).toBe("destructive");
  });
});
