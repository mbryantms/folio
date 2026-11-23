# Dropdown menus

The rules for every dropdown menu in the web app: gear menus, kebabs,
labelled `▾` buttons and the account menu. Menus come from
[`components/folio/dropdown-menu.tsx`](../../web/components/folio/dropdown-menu.tsx),
which wraps the vendored shadcn file. App code never imports
`components/ui/dropdown-menu` directly (ESLint enforces this). The shared
defaults live in the wrapper and in `globals.css`, so the vendored file
stays pristine for `shadcn` refreshes.

Guards are in
[`web/tests/dom/menu-standard.test.tsx`](../../web/tests/dom/menu-standard.test.tsx).

## Width: size to the labels

`DropdownMenuContent` and `DropdownMenuSubContent` default to
`MENU_CONTENT_WIDTH`:

- **Default:** the menu is as wide as its longest label (`w-auto`).
- **Floor:** never narrower than 12rem (192px).
- **Cap:** never wider than 20rem (320px), or the room left on screen. A
  label wraps only past that cap. `overflow-wrap: anywhere` breaks a URL or
  path instead of widening the menu.

The vendored default is the trigger's width (min 128px). A menu opened
from a 36px kebab was therefore squeezed to 128–160px, and labels like
"Fetch metadata for view" wrapped even on a desktop.

**Don't set a width on a menu.** The exception is a menu whose items
carry a description line, which sets a floor that suits the
descriptions. Today that's the reader's Markers / More menus
(`min-w-[18rem]`) and the admin Scan mode menu (`w-80`). A caller's
`w-*` / `min-w-*` / `max-w-*` wins over the default (tailwind-merge).

**Unbounded labels** (a series or issue name, a group path) go in a
`<span className="min-w-0 truncate" title={…}>` so they stay on one line
with the full text on hover. Examples: the cover kebab's
"Read ⟨name⟩" and the views board's "Move to…" paths.

## Structure

- **Group, then label the group.** A menu with more than about six items
  is split into groups with `DropdownMenuLabel` headings. The gear menus
  use Reading / Library / Admin. Admin-only items go in their own group
  at the end, never mixed in with reader items. Don't open a menu with a
  heading that only repeats the trigger (the old "Actions" heading).
- **Separators only divide.** `globals.css` ("Menu separators") hides a
  separator that would sit first or last in a menu or group, or right
  after another separator. So conditional items never leave a dangling
  or doubled line, and callers don't need to guard every
  `<DropdownMenuSeparator />`.
- **Submenus** go through `DropdownMenuResponsiveSub`:
  - On a pointer device it is a side submenu.
  - On touch it is flattened inline under its label and **fenced by a
    separator on each side**. Without the closing fence, the items that
    followed it read as part of it ("Metadata" under "Specials in
    completion", "Edit series" under "Thumbnails").
  - Use a submenu for a family of three or more related items, or for a
    radio choice. Don't use one for a single action.
- **Destructive actions come last, after a separator,** and use
  `variant="destructive"`. Never hand-write `text-destructive` classes.
  - Menus built from an action list (cover kebab, marker rows, selection
    toolbar) split the list with
    [`splitDestructive`](../../web/components/folio/menu-actions.ts).
  - A destructive member of a submenu family (e.g. "Rebuild all page
    thumbnails" in Thumbnails) sits last in that submenu, after its own
    separator.

## Items

- **Icons:** either every item in a menu has one or none does. The item's
  own `gap-2` spaces the icon, so don't add `mr-2`. Icons are `size-4`.
- **Shortcut hints** are right-aligned: `DropdownMenuShortcut`, or a
  `<Kbd>` with `ml-auto` in a full-width title row for the reader's
  described items.
- **Disabled items need a reason the user can see.** Show it as a second
  line under the label (`MenuItemText` in `GroupedViewsBoard`), or keep
  the item enabled and explain in a toast on select. Want to Read does
  this while its collection is still loading.
- **Toggle rows** say what they will do ("Hide from sidebar" / "Show in
  sidebar"). They don't also carry a checkmark for the current state.
  Checkmarks belong to `DropdownMenuCheckboxItem` / `DropdownMenuRadioItem`.
- **Copy:** use sentence case and an ellipsis when the item opens a
  dialog ("Add to collection…"). Use the same words as the keyboard
  shortcuts sheet ("Read in incognito", "Mark all as read").
- **Row height** stays shadcn's 32px on every pointer. This matches the
  touch standard in [`phone-layout.md`](phone-layout.md): visible sizes
  are the same on every device.
