"use client";

import * as React from "react";
import {
  Check,
  ListChecks,
  MessageSquare,
  MoreHorizontal,
  PanelLeft,
  PanelLeftClose,
  Trash2,
  X,
} from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/folio/dropdown-menu";
import { useTogglePageSidebar, useUpdatePage } from "@/lib/api/mutations";
import { useDeletePageWithUndo } from "@/lib/pages/use-delete-page-undo";

import { EditDescriptionDialog } from "./EditDescriptionDialog";
import { ManagePinsDialog } from "./ManagePinsDialog";

/** Multi-page rails follow-up — kebab menu rendered in the page
 *  toolbar (next to the search box + density toggle) on
 *  `/pages/[slug]`. Replaces the kebab that used to live in the
 *  PageHeading next to the title. Items:
 *
 *    - Description: opens a dialog to add / edit / clear the
 *      free-form descriptor rendered under the page title.
 *    - Show in sidebar: toggles the page's sidebar visibility via
 *      `POST /me/pages/{id}/sidebar`. System pages omit this item.
 *    - Manage rails: opens a per-page picker showing every saved
 *      view; toggling a checkbox pins/unpins it on this page.
 *    - Delete page: deletes at once with an Undo toast that rebuilds
 *      the page (name, description, rails, sidebar, slot).
 *
 *  System pages render a stripped-down menu — they can't be deleted
 *  and don't have a sidebar override (the builtin Home entry does). */
export function PageActionsMenu({
  pageId,
  pageDescription,
  isSystem,
  showInSidebar,
}: {
  pageId: string;
  pageDescription: string | null;
  isSystem: boolean;
  showInSidebar: boolean;
}) {
  const updatePage = useUpdatePage(pageId);
  const toggleSidebar = useTogglePageSidebar(pageId);
  const del = useDeletePageWithUndo(pageId, { leaveRoute: true });
  const [descOpen, setDescOpen] = React.useState(false);
  const [pinOpen, setPinOpen] = React.useState(false);

  const hasDescription =
    typeof pageDescription === "string" && pageDescription.length > 0;

  return (
    <>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            type="button"
            variant="outline"
            size="icon"
            className="h-9 w-9 shrink-0"
            aria-label="Page actions"
            title="Page actions"
          >
            <MoreHorizontal className="size-4" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="min-w-[14rem]">
          <DropdownMenuItem
            onSelect={(e) => {
              e.preventDefault();
              setDescOpen(true);
            }}
          >
            <MessageSquare className="mr-2 size-4" />
            {hasDescription ? "Edit description" : "Add description"}
          </DropdownMenuItem>
          {hasDescription ? (
            <DropdownMenuItem
              onSelect={(e) => {
                e.preventDefault();
                updatePage.mutate({ description: "" });
              }}
            >
              <X className="mr-2 size-4" />
              Clear description
            </DropdownMenuItem>
          ) : null}
          {!isSystem && (
            <DropdownMenuItem
              onSelect={(e) => {
                e.preventDefault();
                toggleSidebar.mutate({ show: !showInSidebar });
              }}
            >
              {showInSidebar ? (
                <>
                  <PanelLeftClose className="mr-2 size-4" />
                  Hide from sidebar
                </>
              ) : (
                <>
                  <PanelLeft className="mr-2 size-4" />
                  Show in sidebar
                </>
              )}
              {showInSidebar ? (
                <Check className="text-muted-foreground ml-auto size-3.5" />
              ) : null}
            </DropdownMenuItem>
          )}
          <DropdownMenuItem
            onSelect={(e) => {
              e.preventDefault();
              setPinOpen(true);
            }}
          >
            <ListChecks className="mr-2 size-4" />
            Manage rails…
          </DropdownMenuItem>
          {!isSystem && (
            <>
              <DropdownMenuSeparator />
              <DropdownMenuItem
                disabled={del.isPending}
                onSelect={() => void del.run()}
                className="text-destructive focus:text-destructive"
              >
                <Trash2 className="mr-2 size-4" />
                Delete page
              </DropdownMenuItem>
            </>
          )}
        </DropdownMenuContent>
      </DropdownMenu>
      <EditDescriptionDialog
        open={descOpen}
        onOpenChange={setDescOpen}
        pageId={pageId}
        initial={pageDescription ?? ""}
      />
      <ManagePinsDialog
        open={pinOpen}
        onOpenChange={setPinOpen}
        pageId={pageId}
      />
    </>
  );
}
