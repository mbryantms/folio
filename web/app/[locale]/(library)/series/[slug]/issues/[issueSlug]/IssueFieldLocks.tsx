"use client";

/**
 * Per-field lock on the issue Metadata tab (audit 2026-11 §5 #5). A locked
 * field keeps its current value through provider applies and rescans — the
 * same server-side pin an edit of the field writes, without editing it.
 * Admins get one lock toggle per lockable field; everyone else sees which
 * fields are locked. The control says what the lock does, never who set it.
 */

import { Lock, LockOpen } from "lucide-react";

import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/folio/tooltip";
import { Button } from "@/components/ui/button";
import { useSetIssueFieldLock } from "@/lib/api/mutations";
import { metadataFieldLabel } from "@/lib/metadata-fields";
import { cn } from "@/lib/utils";

const LOCKED_HINT = "Locked: provider updates won't change this field";
const UNLOCKED_HINT = "Unlocked: provider updates can change this field";

export function IssueFieldLocks({
  seriesSlug,
  issueSlug,
  lockableFields,
  lockedFields,
  otherLockedFields,
  canEdit,
}: {
  seriesSlug: string;
  issueSlug: string;
  /** `MetadataOverviewView.lockable_fields`, in display order. */
  lockableFields: string[];
  /** `MetadataOverviewView.locked_fields`. */
  lockedFields: string[];
  /** `MetadataOverviewView.other_locked_fields` — pins outside the
   *  lockable set; listed while they hold so they can be released. */
  otherLockedFields: string[];
  canEdit: boolean;
}) {
  const setLock = useSetIssueFieldLock(seriesSlug, issueSlug);
  const locked = new Set([...lockedFields, ...otherLockedFields]);
  const fields = [
    ...(canEdit ? lockableFields : lockableFields.filter((f) => locked.has(f))),
    ...otherLockedFields,
  ];
  if (fields.length === 0) return null;

  return (
    <section className="space-y-2" aria-labelledby="issue-field-locks-heading">
      <h3
        id="issue-field-locks-heading"
        className="text-foreground text-sm font-semibold"
      >
        Field locks
      </h3>
      <div className="border-border bg-card space-y-3 rounded-lg border p-4">
        <p className="text-muted-foreground text-sm">
          A locked field keeps its current value when provider metadata is
          applied or the file is rescanned.
          {canEdit && " Editing a field locks it too."}
        </p>
        <ul
          className="grid grid-cols-1 gap-x-6 gap-y-1 sm:grid-cols-2 lg:grid-cols-3"
          aria-busy={setLock.isPending || undefined}
        >
          {fields.map((field) => {
            const label = metadataFieldLabel(field);
            const isLocked = locked.has(field);
            const icon = isLocked ? (
              // Filled body = locked; the open outline = unlocked.
              <Lock className="size-4 [&>rect]:fill-current" aria-hidden />
            ) : (
              <LockOpen className="size-4" aria-hidden />
            );
            return (
              <li
                key={field}
                className="flex items-center justify-between gap-2"
              >
                <span
                  className={cn(
                    "truncate text-sm",
                    isLocked ? "text-foreground" : "text-muted-foreground",
                  )}
                >
                  {label}
                </span>
                {canEdit ? (
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <Button
                        type="button"
                        variant="ghost"
                        size="icon-sm"
                        aria-label={`Lock ${label} against provider updates`}
                        aria-pressed={isLocked}
                        disabled={setLock.isPending}
                        className={cn(
                          isLocked
                            ? "text-foreground"
                            : "text-muted-foreground",
                        )}
                        onClick={() =>
                          setLock.mutate({ field, locked: !isLocked })
                        }
                      >
                        {icon}
                      </Button>
                    </TooltipTrigger>
                    <TooltipContent>
                      {isLocked ? LOCKED_HINT : UNLOCKED_HINT}
                    </TooltipContent>
                  </Tooltip>
                ) : (
                  <span
                    role="img"
                    aria-label={LOCKED_HINT}
                    title={LOCKED_HINT}
                    className="text-foreground inline-flex size-8 items-center justify-center"
                  >
                    {icon}
                  </span>
                )}
              </li>
            );
          })}
        </ul>
      </div>
    </section>
  );
}
