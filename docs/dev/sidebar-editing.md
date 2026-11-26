# Editing the sidebar in place

"Edit sidebar" in the account menu at the foot of the library sidebar
([`MainSidebar.tsx`](../../web/components/library/MainSidebar.tsx),
`UserFooter`) swaps
the links for
[`SidebarEditMode`](../../web/components/library/SidebarEditMode.tsx) in
the same column (and in the phone nav sheet). It draws the layout as the
sidebar does: headers as section labels, items with their icons. Each row
has a `touch-none` grip and a ⋯ menu (Rename, Move up / down, Add header
or spacer above, Hide). Hidden rows sit in a "Hidden" list with a Show
button. In the icon-collapsed rail the button expands the rail first.

- The edits are pure functions in
  [`lib/sidebar-layout/edit.ts`](../../web/lib/sidebar-layout/edit.ts)
  over `{visible, hidden}`; `useSidebarLayoutEdit` saves each one at once
  with the whole-list PATCH. Hidden rows are always in the payload
  (leaving them out would un-hide them).
- Taking a row out hides it, except a header or spacer the user made
  (`isCustomDivider`: not a `default:` ref id), which is deleted with an
  Undo toast.
- The editor is loaded with `next/dynamic` on first use, so drag and drop
  adds nothing to every page's bundle.
- Settings → Sidebar remains for adding saved views that were never in
  the sidebar and for Reset. It has its own copy of the save logic
  (`NavigationManager.tsx`); moving it onto `lib/sidebar-layout/edit.ts`
  is open follow-up work.
