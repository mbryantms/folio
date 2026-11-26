"use client";

/**
 * Editing the sidebar in place (inline sidebar editing).
 *
 * "Edit sidebar" in the account menu (foot of the sidebar) swaps the links
 * for this editor, in the same column and in the same order and grouping: headers
 * read as section labels, items keep their icons. Each row has a grip
 * (drag, touch included) and a ⋯ menu: Rename, Move up / down, Add header
 * or spacer above, Hide. Hidden items sit in a "Hidden" list at the
 * bottom with a Show button. Every change saves at once.
 *
 * Settings → Sidebar stays for what does not fit a 16rem column: adding
 * saved views that were never in the sidebar, and Reset.
 */

import {
  DndContext,
  KeyboardSensor,
  PointerSensor,
  closestCenter,
  useSensor,
  useSensors,
  type DragEndEvent,
} from "@dnd-kit/core";
import {
  SortableContext,
  sortableKeyboardCoordinates,
  useSortable,
  verticalListSortingStrategy,
} from "@dnd-kit/sortable";
import {
  ArrowDown,
  ArrowUp,
  Eye,
  EyeOff,
  GripVertical,
  Heading,
  Minus,
  MoreHorizontal,
  Pencil,
  Sparkles,
  Trash2,
} from "lucide-react";
import Link from "next/link";
import * as React from "react";

import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/folio/dropdown-menu";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import type { SidebarEntryView } from "@/lib/api/types";
import { isCustomDivider, keyOf } from "@/lib/sidebar-layout/edit";
import {
  useSidebarLayoutEdit,
  type SidebarLayoutEdit,
} from "@/lib/sidebar-layout/use-layout-edit";
import {
  DRAG_LIFT_CLASS,
  sortableStyle,
  useSpringSort,
} from "@/lib/ui/springy-drag";
import { cn } from "@/lib/utils";

import { mainNavIcons } from "./main-nav-icons";
import { railIconByKey } from "./rail-icons";

function EntryIcon({ entry }: { entry: SidebarEntryView }) {
  const Icon =
    mainNavIcons[entry.icon as keyof typeof mainNavIcons] ??
    railIconByKey(entry.icon)?.Icon ??
    Sparkles;
  return <Icon className="size-4 shrink-0" aria-hidden />;
}

/** What a row is called in button labels. */
function rowName(entry: SidebarEntryView): string {
  return entry.kind === "spacer" ? "spacer" : entry.label;
}

export function SidebarEditMode({ onDone }: { onDone: () => void }) {
  const edit = useSidebarLayoutEdit();
  const sensors = useSensors(
    // The grip is `touch-none`: a finger on it drags, the rest scrolls.
    useSensor(PointerSensor, { activationConstraint: { distance: 4 } }),
    useSensor(KeyboardSensor, {
      coordinateGetter: sortableKeyboardCoordinates,
    }),
  );
  const onDragEnd = (e: DragEndEvent) => {
    const { active, over } = e;
    if (!over || active.id === over.id) return;
    edit.reorder(String(active.id), String(over.id));
  };
  const keys = edit.visible.map(keyOf);

  return (
    <section
      aria-label="Edit sidebar"
      data-sidebar-edit
      className="flex flex-col gap-2 p-2"
    >
      <div className="flex items-center justify-between gap-2 px-1">
        <h2 className="text-sidebar-foreground text-sm font-semibold">
          Edit sidebar
        </h2>
        <Button type="button" size="sm" onClick={onDone}>
          Done
        </Button>
      </div>
      {edit.isLoading ? (
        <p className="text-muted-foreground px-1 text-xs">Loading…</p>
      ) : edit.isError ? (
        <p className="text-destructive px-1 text-xs">
          Couldn&rsquo;t load the sidebar layout.
        </p>
      ) : (
        <>
          <DndContext
            sensors={sensors}
            collisionDetection={closestCenter}
            onDragEnd={onDragEnd}
          >
            <SortableContext
              items={keys}
              strategy={verticalListSortingStrategy}
            >
              <ol className="flex flex-col gap-0.5">
                {edit.visible.map((entry, index) => (
                  <EditRow
                    key={keyOf(entry)}
                    entry={entry}
                    index={index}
                    count={edit.visible.length}
                    edit={edit}
                  />
                ))}
              </ol>
            </SortableContext>
          </DndContext>
          <div className="flex flex-wrap gap-1.5 px-1 pt-1">
            <Button
              type="button"
              size="sm"
              variant="outline"
              onClick={() => edit.addDivider("header", null)}
            >
              <Heading aria-hidden />
              Header
            </Button>
            <Button
              type="button"
              size="sm"
              variant="outline"
              onClick={() => edit.addDivider("spacer", null)}
            >
              <Minus aria-hidden />
              Spacer
            </Button>
          </div>
          {edit.hidden.length > 0 ? (
            <div className="pt-2">
              <h3 className="text-sidebar-foreground/70 px-2 pb-1 text-xs font-medium tracking-widest uppercase">
                Hidden
              </h3>
              <ul className="flex flex-col gap-0.5">
                {edit.hidden.map((entry) => (
                  <li
                    key={keyOf(entry)}
                    className="text-sidebar-foreground/70 flex h-8 items-center gap-2 rounded-md pr-1 pl-2 text-sm"
                  >
                    {entry.kind === "header" ? null : (
                      <EntryIcon entry={entry} />
                    )}
                    <span
                      className={cn(
                        "min-w-0 flex-1 truncate",
                        entry.kind === "header" &&
                          "text-xs tracking-widest uppercase",
                      )}
                      title={entry.label}
                    >
                      {entry.label}
                    </span>
                    <Button
                      type="button"
                      size="icon"
                      variant="ghost"
                      className="size-7 shrink-0"
                      aria-label={`Show ${entry.label} in the sidebar`}
                      title="Show in sidebar"
                      onClick={() => edit.showAgain(entry)}
                    >
                      <Eye className="size-4" aria-hidden />
                    </Button>
                  </li>
                ))}
              </ul>
            </div>
          ) : null}
          <p className="text-muted-foreground px-1 pt-2 text-xs">
            Changes save as you go.{" "}
            <Link
              href="/settings/sidebar"
              className="text-foreground underline-offset-4 hover:underline"
              onClick={onDone}
            >
              More options
            </Link>{" "}
            (add saved views, reset).
          </p>
        </>
      )}
    </section>
  );
}

function EditRow({
  entry,
  index,
  count,
  edit,
}: {
  entry: SidebarEntryView;
  index: number;
  count: number;
  edit: SidebarLayoutEdit;
}) {
  const {
    attributes,
    listeners,
    setNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({ id: keyOf(entry), transition: useSpringSort() });
  const [renaming, setRenaming] = React.useState(false);
  const [draft, setDraft] = React.useState(entry.label);
  const isHeader = entry.kind === "header";
  const isSpacer = entry.kind === "spacer";
  const name = rowName(entry);
  const finishRename = (save: boolean) => {
    setRenaming(false);
    if (save) edit.rename(entry, draft);
  };

  return (
    <li
      ref={setNodeRef}
      style={sortableStyle({ transform, transition, isDragging })}
      data-kind={entry.kind}
      className={cn(
        "bg-sidebar flex h-8 items-center gap-1 rounded-md text-sm",
        // A header starts a section: a little air above it, like the
        // real sidebar's groups.
        isHeader && index > 0 && "mt-2",
        isDragging && DRAG_LIFT_CLASS,
      )}
    >
      <button
        type="button"
        aria-label={`Drag ${name}`}
        className="text-muted-foreground hover:text-foreground hover:bg-sidebar-accent flex size-7 shrink-0 cursor-grab touch-none items-center justify-center rounded-md active:cursor-grabbing"
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        {...(attributes as any)}
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        {...(listeners as any)}
      >
        <GripVertical className="size-4" aria-hidden />
      </button>
      {isSpacer ? (
        <span className="border-sidebar-foreground/25 min-w-0 flex-1 border-t border-dashed" />
      ) : renaming ? (
        <Input
          autoFocus
          aria-label={`Rename ${entry.label}`}
          value={draft}
          maxLength={80}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={() => finishRename(true)}
          onKeyDown={(e) => {
            if (e.key === "Enter") finishRename(true);
            if (e.key === "Escape") finishRename(false);
          }}
          className="h-7 min-w-0 flex-1 px-1.5 text-sm"
        />
      ) : (
        <>
          {isHeader ? null : <EntryIcon entry={entry} />}
          <span
            className={cn(
              "min-w-0 flex-1 truncate",
              isHeader
                ? "text-sidebar-foreground/70 text-xs font-medium tracking-widest uppercase"
                : "pl-1",
            )}
            title={entry.label}
          >
            {entry.label}
          </span>
        </>
      )}
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            type="button"
            size="icon"
            variant="ghost"
            className="text-muted-foreground size-7 shrink-0"
            aria-label={`${name} options`}
          >
            <MoreHorizontal className="size-4" aria-hidden />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end">
          {isSpacer ? null : (
            <DropdownMenuItem
              onSelect={() => {
                setDraft(entry.label);
                setRenaming(true);
              }}
            >
              <Pencil className="size-4" aria-hidden />
              Rename
            </DropdownMenuItem>
          )}
          <DropdownMenuItem
            disabled={index === 0}
            onSelect={() => edit.moveBy(entry, -1)}
          >
            <ArrowUp className="size-4" aria-hidden />
            Move up
          </DropdownMenuItem>
          <DropdownMenuItem
            disabled={index >= count - 1}
            onSelect={() => edit.moveBy(entry, 1)}
          >
            <ArrowDown className="size-4" aria-hidden />
            Move down
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem onSelect={() => edit.addDivider("header", entry)}>
            <Heading className="size-4" aria-hidden />
            Add header above
          </DropdownMenuItem>
          <DropdownMenuItem onSelect={() => edit.addDivider("spacer", entry)}>
            <Minus className="size-4" aria-hidden />
            Add spacer above
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          {isCustomDivider(entry) ? (
            <DropdownMenuItem
              variant="destructive"
              onSelect={() => edit.takeOut(entry)}
            >
              <Trash2 className="size-4" aria-hidden />
              Remove {isHeader ? "header" : "spacer"}
            </DropdownMenuItem>
          ) : (
            <DropdownMenuItem onSelect={() => edit.takeOut(entry)}>
              <EyeOff className="size-4" aria-hidden />
              Hide from sidebar
            </DropdownMenuItem>
          )}
        </DropdownMenuContent>
      </DropdownMenu>
    </li>
  );
}
