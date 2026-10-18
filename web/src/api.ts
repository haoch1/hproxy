let csrf = "";
export const demoMode =
  window.location.pathname === "/demo" ||
  window.location.pathname.startsWith("/demo/");
export function setCsrf(value: string) {
  csrf = value;
}
export async function api<T>(
  path: string,
  options: RequestInit = {},
): Promise<T> {
  if (demoMode) {
    const { demoApi } = await import("./demo");
    return demoApi<T>(path, options);
  }
  const controller = new AbortController();
  const timer = window.setTimeout(() => controller.abort(), 20000);
  try {
    const response = await fetch(`/api/v1${path}`, {
      ...options,
      credentials: "same-origin",
      headers: {
        "Content-Type": "application/json",
        "X-CSRF-Token": csrf,
        ...options.headers,
      },
      signal: controller.signal,
    });
    if (response.status === 401 && path != "/login")
      window.dispatchEvent(new Event("session-expired"));
    const data = await response
      .json()
      .catch(() => ({ error: `请求失败 (${response.status})` }));
    if (!response.ok) throw new Error(data.error || "请求失败");
    return data as T;
  } catch (e) {
    if (e instanceof DOMException && e.name === "AbortError")
      throw new Error("连接超时，请检查主控服务");
    throw e;
  } finally {
    clearTimeout(timer);
  }
}
export const post = <T>(path: string, body: unknown = {}) =>
  api<T>(path, { method: "POST", body: JSON.stringify(body) });
export const put = <T>(path: string, body: unknown) =>
  api<T>(path, { method: "PUT", body: JSON.stringify(body) });
export const remove = (path: string) => api(path, { method: "DELETE" });

export async function checkUpdatesOnEntry() {
  const status = await api<{ source_configured: boolean; phase: string }>("/updates");
  if (!status.source_configured || ["checking", "downloading", "restarting", "verifying"].includes(status.phase)) return;
  await post("/updates/check");
  window.dispatchEvent(new Event("updates-changed"));
}
