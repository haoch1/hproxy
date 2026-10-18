/** Subscribe to committed log changes. Reconnect catches up through a fresh page query. */
export function subscribeLogUpdates(reload: () => void) {
  let connected = false;
  let disposed = false;
  let source: EventSource | undefined;
  let batch: ReturnType<typeof setTimeout> | undefined;
  const schedule = () => {
    if (disposed || batch !== undefined) return;
    batch = setTimeout(() => { batch = undefined; reload(); }, 80);
  };
  if (typeof EventSource !== "undefined") {
    try {
      source = new EventSource("/api/v1/logs/events");
      source.addEventListener("ready", () => { connected = true; schedule(); });
      source.addEventListener("logs", schedule);
      source.addEventListener("error", () => { connected = false; });
    } catch { /* Poll while the streaming transport is unavailable. */ }
  }
  const fallback = setInterval(() => { if (!connected) schedule(); }, 3000);
  const visible = () => { if (!document.hidden) schedule(); };
  document.addEventListener("visibilitychange", visible);
  window.addEventListener("online", schedule);
  return () => {
    disposed = true;
    source?.close();
    clearInterval(fallback);
    clearTimeout(batch);
    document.removeEventListener("visibilitychange", visible);
    window.removeEventListener("online", schedule);
  };
}

/** One request at a time; a burst during a request needs only one catch-up query. */
export function liveReload<T>(read: () => Promise<T>, commit: (value: T) => void, fail: (error: unknown) => void) {
  let closed = false, running = false, queued = false;
  let retry: ReturnType<typeof setTimeout> | undefined;
  const reload = async () => {
    if (closed) return;
    if (running) { queued = true; return; }
    clearTimeout(retry);
    running = true;
    try {
      const value = await read();
      if (!closed) commit(value);
    } catch (error) {
      if (!closed) {
        fail(error);
        retry = setTimeout(() => { void reload(); }, 3000);
      }
    } finally {
      running = false;
      if (!closed && queued) { queued = false; void reload(); }
    }
  };
  return {
    reload: () => { void reload(); },
    stop: () => { closed = true; clearTimeout(retry); },
  };
}
