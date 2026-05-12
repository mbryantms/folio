"use client";

import { ChevronDown, Gauge, Play, RefreshCw, ShieldCheck } from "lucide-react";
import type { LucideIcon } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import type { ScanMode } from "@/lib/api/types";

const MODES: {
  mode: ScanMode;
  label: string;
  cost: string;
  description: string;
  icon: LucideIcon;
}[] = [
  {
    mode: "normal",
    label: "Normal scan",
    cost: "Low cost",
    description: "Verify folders, ingest changed files, enqueue stale covers.",
    icon: Play,
  },
  {
    mode: "metadata_refresh",
    label: "Metadata refresh",
    cost: "Medium cost",
    description:
      "Refresh sidecar and archive metadata without full content checks.",
    icon: RefreshCw,
  },
  {
    mode: "content_verify",
    label: "Content verify",
    cost: "High cost",
    description: "Re-read archive content when file identity may be stale.",
    icon: ShieldCheck,
  },
];

export function ScanModeMenu({
  disabled,
  isPending,
  isRunning,
  onScan,
}: {
  disabled?: boolean;
  isPending?: boolean;
  isRunning?: boolean;
  onScan: (mode: ScanMode) => void;
}) {
  const blocked = disabled || isPending || isRunning;
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button disabled={blocked} className="gap-2">
          {isPending || isRunning ? (
            <Gauge className="h-4 w-4 animate-pulse" />
          ) : (
            <Play className="h-4 w-4" />
          )}
          {isRunning ? "Scan running" : isPending ? "Queueing" : "Scan"}
          <ChevronDown className="h-4 w-4 opacity-70" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-80">
        <DropdownMenuLabel>Choose scan mode</DropdownMenuLabel>
        <DropdownMenuSeparator />
        {MODES.map((item) => {
          const Icon = item.icon;
          return (
            <DropdownMenuItem
              key={item.mode}
              className="items-start gap-3 py-2"
              onSelect={() => onScan(item.mode)}
            >
              <Icon className="mt-0.5 h-4 w-4 shrink-0" />
              <span className="min-w-0 space-y-0.5">
                <span className="flex items-center gap-2">
                  <span className="font-medium">{item.label}</span>
                  <span className="text-muted-foreground text-[11px]">
                    {item.cost}
                  </span>
                </span>
                <span className="text-muted-foreground block text-xs whitespace-normal">
                  {item.description}
                </span>
              </span>
            </DropdownMenuItem>
          );
        })}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
