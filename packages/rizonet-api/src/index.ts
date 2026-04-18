/**
 * @rizonet/api — TypeScript bindings for the Rizonet IPC bridge.
 *
 * The Rust host injects `window.__RIZONET__` into every webview.
 * This package provides a small, typed facade (`invoke`, `isRizonet`,
 * `version`, `createClient`) around that global.
 */

declare global {
  interface Window {
    __RIZONET__?: {
      version: string;
      invoke: <T = unknown>(cmd: string, payload?: unknown) => Promise<T>;
    };
  }
}

export function isRizonet(): boolean {
  return typeof window !== "undefined" && !!window.__RIZONET__;
}

export function version(): string | null {
  return window.__RIZONET__?.version ?? null;
}

export function invoke<T = unknown>(cmd: string, payload?: unknown): Promise<T> {
  const rz = window.__RIZONET__;
  if (!rz) {
    return Promise.reject(
      new Error(
        "Rizonet bridge not available. Are you running inside a Rizonet app?"
      )
    );
  }
  return rz.invoke<T>(cmd, payload);
}

/**
 * Build a type-safe invoke client from a mapping of command names to
 * `{ args, returns }` shapes.
 */
export function createClient<
  Commands extends Record<string, { args?: unknown; returns: unknown }>
>() {
  return {
    invoke<K extends keyof Commands & string>(
      cmd: K,
      payload?: Commands[K]["args"]
    ): Promise<Commands[K]["returns"]> {
      return invoke<Commands[K]["returns"]>(cmd, payload);
    },
  };
}
