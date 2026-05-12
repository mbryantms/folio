/**
 * `writeThemeCookie` and friends only exist on the client. This test fakes
 * a `document` for the duration of the run so we can assert the exact
 * `Set-Cookie`-style strings without bringing in jsdom.
 */
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import {
  THEME_COOKIE,
  ACCENT_COOKIE,
  DENSITY_COOKIE,
  writeAccentCookie,
  writeDensityCookie,
  writeThemeCookie,
} from "@/lib/theme";

const writes: string[] = [];

beforeEach(() => {
  writes.length = 0;
  Object.defineProperty(globalThis, "document", {
    configurable: true,
    value: {
      get cookie() {
        return "";
      },
      set cookie(v: string) {
        writes.push(v);
      },
    },
  });
});

afterEach(() => {
  // @ts-expect-error fake document goes away
  delete globalThis.document;
});

describe("theme cookie writers", () => {
  it("encodes the value with the documented cookie name + Path", () => {
    writeThemeCookie("dark");
    expect(writes[0]).toContain(`${THEME_COOKIE}=dark`);
    expect(writes[0]).toContain("Path=/");
    expect(writes[0]).toContain("SameSite=Lax");
  });

  it("clears the cookie when value is null", () => {
    writeAccentCookie(null);
    expect(writes[0]).toContain(`${ACCENT_COOKIE}=`);
    expect(writes[0]).toContain("Max-Age=0");
  });

  it("writes density too", () => {
    writeDensityCookie("compact");
    expect(writes[0]).toContain(`${DENSITY_COOKIE}=compact`);
  });
});
