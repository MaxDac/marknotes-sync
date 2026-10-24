// Recovery from a trap in the WebAssembly instance. The core does not panic on any input (the fuzz targets check
// this), but if it ever did, the trap (a WebAssembly.RuntimeError) would leave the instance's memory and stack
// half-updated, and every later call could fail or return wrong results. `guard` wraps a function so that after
// such an error the instance is replaced with a fresh one before the error is rethrown. SyncScheduler objects
// created before the reset then throw "Invalid stale object from previous Wasm instance" instead of using the old
// state. Other errors (invalid arguments) are rethrown unchanged.

export function isTrap(error) {
  return typeof WebAssembly !== "undefined" && error instanceof WebAssembly.RuntimeError;
}

export function guard(fn, reset) {
  return function guarded(...args) {
    try {
      return fn.apply(this, args);
    } catch (error) {
      if (isTrap(error)) reset();
      throw error;
    }
  };
}
