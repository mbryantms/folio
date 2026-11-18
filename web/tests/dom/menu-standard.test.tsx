// @vitest-environment jsdom
/**
 * The dropdown-menu standard (docs/dev/menus.md):
 *  - every menu sizes to its labels (192px floor, 20rem cap) instead of the
 *    vendored "as wide as the trigger" default, which squeezed a kebab's
 *    menu to 128–160px and wrapped its labels;
 *  - a submenu flattened inline on touch is fenced by separators so the
 *    items after it don't read as part of it;
 *  - destructive actions in list-built menus go last, after a separator.
 */
import { render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

let touch = false;
vi.mock("@/lib/ui/use-coarse-pointer", async (importOriginal) => ({
  ...(await importOriginal<Record<string, unknown>>()),
  useTouchDevice: () => touch,
}));

import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
  MENU_CONTENT_WIDTH,
} from "@/components/folio/dropdown-menu";
import { DropdownMenuResponsiveSub } from "@/components/folio/dropdown-menu-responsive-sub";
import { splitDestructive } from "@/components/folio/menu-actions";

beforeEach(() => {
  touch = false;
});

function OpenMenu({
  className,
  children,
}: {
  className?: string;
  children: React.ReactNode;
}) {
  return (
    <DropdownMenu open>
      <DropdownMenuTrigger>Open</DropdownMenuTrigger>
      <DropdownMenuContent className={className}>
        {children}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

describe("menu width", () => {
  it("sizes to content between 12rem and 20rem, not to the trigger", () => {
    render(
      <OpenMenu>
        <DropdownMenuItem>Fetch metadata for view</DropdownMenuItem>
      </OpenMenu>,
    );
    const menu = screen.getByRole("menu");
    expect(menu.className).toContain("w-auto");
    expect(menu.className).toContain("min-w-48");
    expect(menu.className).toContain("max-w-[min(20rem,");
    expect(menu.className).not.toContain("--radix-dropdown-menu-trigger-width");
    expect(menu.className).not.toMatch(/(^|\s)min-w-32(\s|$)/);
    expect(MENU_CONTENT_WIDTH).toContain("min-w-48");
  });

  it("still lets a menu with description lines set its own width", () => {
    render(
      <OpenMenu className="min-w-[18rem]">
        <DropdownMenuItem>Add note</DropdownMenuItem>
      </OpenMenu>,
    );
    const menu = screen.getByRole("menu");
    expect(menu.className).toContain("min-w-[18rem]");
    expect(menu.className).not.toMatch(/(^|\s)min-w-48(\s|$)/);
  });
});

describe("responsive submenu", () => {
  it("fences the flattened section with separators on touch", () => {
    touch = true;
    render(
      <OpenMenu>
        <DropdownMenuItem>Copy link</DropdownMenuItem>
        <DropdownMenuResponsiveSub label="Thumbnails">
          <DropdownMenuItem>Rebuild cover</DropdownMenuItem>
        </DropdownMenuResponsiveSub>
        <DropdownMenuItem>Edit series</DropdownMenuItem>
      </OpenMenu>,
    );
    const menu = screen.getByRole("menu");
    const order = [...menu.children].map(
      (el) =>
        el.getAttribute("data-slot")?.replace("dropdown-menu-", "") +
        (el.textContent ? `:${el.textContent}` : ""),
    );
    expect(order).toEqual([
      "item:Copy link",
      "separator",
      "label:Thumbnails",
      "item:Rebuild cover",
      "separator",
      "item:Edit series",
    ]);
  });

  it("stays a side submenu on pointer devices", () => {
    render(
      <OpenMenu>
        <DropdownMenuResponsiveSub label="Thumbnails">
          <DropdownMenuItem>Rebuild cover</DropdownMenuItem>
        </DropdownMenuResponsiveSub>
      </OpenMenu>,
    );
    const menu = screen.getByRole("menu");
    expect(
      within(menu).getByRole("menuitem", { name: "Thumbnails" }),
    ).toBeTruthy();
    expect(within(menu).queryByRole("separator")).toBeNull();
  });
});

describe("splitDestructive", () => {
  it("moves destructive actions last, keeping each half's order", () => {
    const { safe, destructive } = splitDestructive([
      { id: "a" },
      { id: "del", destructive: true },
      { id: "b" },
      { id: "purge", destructive: true },
    ]);
    expect(safe.map((a) => a.id)).toEqual(["a", "b"]);
    expect(destructive.map((a) => a.id)).toEqual(["del", "purge"]);
  });
});
