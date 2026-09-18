// @vitest-environment jsdom
import { act, render } from "@testing-library/react";
import { beforeEach, afterEach, expect, it, vi } from "vitest";
const state = vi.hoisted(() => ({
  listeners: {} as Record<string, () => void>,
  register: vi.fn(),
  skip: vi.fn(),
  update: vi.fn(),
  toast: vi.fn(),
}));
vi.mock("@serwist/window", () => ({
  Serwist: class {
    register = state.register;
    update = state.update;
    messageSkipWaiting = state.skip;
    addEventListener(name: string, cb: () => void) {
      state.listeners[name] = cb;
    }
    removeEventListener(name: string) {
      delete state.listeners[name];
    }
  },
}));
vi.mock("sonner", () => ({
  toast: { message: state.toast, error: vi.fn(), dismiss: vi.fn() },
}));
import { ServiceWorkerUpdater } from "@/components/ServiceWorkerUpdater";
beforeEach(() => {
  vi.stubEnv("NODE_ENV", "production");
  state.register.mockResolvedValue(undefined);
  state.update.mockResolvedValue(undefined);
  Object.defineProperty(navigator, "serviceWorker", {
    configurable: true,
    value: {},
  });
});
afterEach(() => {
  vi.unstubAllEnvs();
  vi.clearAllMocks();
});
it("another window's activation offers a choice without auto-accepting", async () => {
  const { unmount } = render(<ServiceWorkerUpdater />);
  await act(async () => {
    state.listeners.controlling!();
  });
  expect(state.skip).not.toHaveBeenCalled();
  const options = state.toast.mock.calls.at(-1)![1];
  expect(options.cancel.label).toBe("Later");
  expect(options.action.label).toBe("Reload");
  unmount();
  expect(state.listeners.controlling).toBeUndefined();
});
it("only asks the waiting worker to activate after acceptance", async () => {
  render(<ServiceWorkerUpdater />);
  await act(async () => {
    state.listeners.waiting!();
  });
  expect(state.skip).not.toHaveBeenCalled();
  await act(async () => {
    state.toast.mock.calls.at(-1)![1].action.onClick();
  });
  expect(state.skip).toHaveBeenCalledOnce();
});
it("does not register a worker in development and removes stale registrations", async () => {
  vi.stubEnv("NODE_ENV", "development");
  const unregister = vi.fn().mockResolvedValue(true);
  Object.defineProperty(navigator, "serviceWorker", {
    configurable: true,
    value: {
      getRegistrations: vi
        .fn()
        .mockResolvedValue([
          { active: { scriptURL: "https://folio.test/sw.js" }, unregister },
        ]),
    },
  });
  render(<ServiceWorkerUpdater />);
  await act(async () => {});
  expect(state.register).not.toHaveBeenCalled();
  expect(unregister).toHaveBeenCalledOnce();
});
