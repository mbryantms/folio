"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";

import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { ShortcutsHelpButton } from "@/components/shell/ShortcutsHelpButton";
import { UserFooter } from "@/components/shell/UserFooter";
import { cn } from "@/lib/utils";

import type { NavSection } from "./nav";
import { navIcons } from "./nav-icons";

export function AdminSidebar({
  sections,
  title,
  user,
  collapsed = false,
}: {
  sections: NavSection[];
  title: string;
  user: { display_name: string; email: string | null; role: string };
  /** When true, the sidebar shrinks to icon-only mode with hover tooltips. */
  collapsed?: boolean;
}) {
  const pathname = usePathname() ?? "";
  return (
    <div className="flex h-full flex-col">
      <TooltipProvider delayDuration={200}>
        <nav
          aria-label={`${title} navigation`}
          className={cn(
            "flex flex-1 flex-col gap-6 overflow-y-auto py-6 text-sm",
            collapsed ? "px-2" : "px-3",
          )}
        >
          {!collapsed && (
            <div className="px-3">
              <p className="text-muted-foreground text-xs font-semibold tracking-widest uppercase">
                {title}
              </p>
            </div>
          )}
          {sections.map((section) => (
            <div key={section.label} className="flex flex-col gap-1">
              {!collapsed && (
                <p className="text-muted-foreground/70 px-3 text-[11px] font-medium tracking-widest uppercase">
                  {section.label}
                </p>
              )}
              <ul className="flex flex-col gap-0.5">
                {section.items.map((item) => {
                  const active =
                    pathname === item.href ||
                    (item.href !== "" && pathname.startsWith(item.href + "/"));
                  const Icon = navIcons[item.icon];
                  const link = (
                    <Link
                      href={item.href}
                      className={cn(
                        "flex items-center rounded-md transition-colors",
                        collapsed
                          ? "size-9 justify-center"
                          : "gap-2.5 px-3 py-1.5",
                        active
                          ? "bg-secondary text-foreground"
                          : "text-muted-foreground hover:bg-secondary/50 hover:text-foreground",
                      )}
                      aria-current={active ? "page" : undefined}
                      aria-label={collapsed ? item.label : undefined}
                    >
                      <Icon className="h-4 w-4 shrink-0" />
                      {!collapsed && (
                        <span className="truncate">{item.label}</span>
                      )}
                      {!collapsed && item.placeholder ? (
                        <span className="text-muted-foreground/60 ml-auto text-[10px] tracking-wider uppercase">
                          soon
                        </span>
                      ) : null}
                    </Link>
                  );
                  return (
                    <li key={item.href}>
                      {collapsed ? (
                        <Tooltip>
                          <TooltipTrigger asChild>{link}</TooltipTrigger>
                          <TooltipContent side="right" sideOffset={8}>
                            {item.label}
                            {item.placeholder ? (
                              <span className="text-muted-foreground ml-2 text-[10px] tracking-wider uppercase">
                                soon
                              </span>
                            ) : null}
                          </TooltipContent>
                        </Tooltip>
                      ) : (
                        link
                      )}
                    </li>
                  );
                })}
              </ul>
            </div>
          ))}
        </nav>
        <div className="px-2 pb-1">
          <ShortcutsHelpButton collapsed={collapsed} />
        </div>
      </TooltipProvider>
      <UserFooter user={user} collapsed={collapsed} />
    </div>
  );
}
