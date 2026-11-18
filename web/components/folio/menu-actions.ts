/**
 * Menu ordering rule (docs/dev/menus.md): destructive actions come last,
 * after a separator. Menus built from an action list (cover kebab, marker
 * rows) split the list with this, keeping each half's own order, and draw a
 * `DropdownMenuSeparator` between the halves when both are non-empty.
 */
export function splitDestructive<T extends { destructive?: boolean }>(
  actions: readonly T[],
): { safe: T[]; destructive: T[] } {
  const safe: T[] = [];
  const destructive: T[] = [];
  for (const a of actions) (a.destructive ? destructive : safe).push(a);
  return { safe, destructive };
}
