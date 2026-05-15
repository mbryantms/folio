"use client";

import {
  Bookmark,
  Calendar,
  Folder,
  Heart,
  Home,
  LayoutGrid,
  Library,
  ListPlus,
  Sparkles,
  type LucideIcon,
} from "lucide-react";

import type { MainNavItem } from "./main-nav";

/**
 * Icon registry for the library/main shell. Server components produce
 * `MainNavItem.icon` as a string name (RSC can't serialize component
 * references), and the client sidebar resolves it through this map.
 */
export const mainNavIcons: Record<MainNavItem["icon"], LucideIcon> = {
  Home,
  Folder,
  ListPlus,
  Bookmark,
  Heart,
  Library,
  Calendar,
  LayoutGrid,
  Sparkles,
  // The remaining IconName values from admin/nav.ts — not used today by the
  // library shell, but having them in the map keeps `MainNavItem.icon: IconName |
  // …` typesafe without admin nav imports needing to know about us.
  Activity: Sparkles,
  BarChart3: Sparkles,
  BookOpen: Sparkles,
  Cog: Sparkles,
  FileClock: Sparkles,
  Gauge: Sparkles,
  Key: Sparkles,
  KeyRound: Sparkles,
  Keyboard: Sparkles,
  ListChecks: Sparkles,
  Palette: Sparkles,
  Search: Sparkles,
  Server: Sparkles,
  Shield: Sparkles,
  UserCog: Sparkles,
  Users: Sparkles,
};
