import { Monitor, Moon, Sun } from "lucide-react";
import type { ReactNode } from "react";
import { createContext, useContext, useEffect, useState } from "react";
import Select from "./Select";

type Theme = "system" | "light" | "dark";
const key = "hproxy-theme";
const ThemeContext = createContext<{
  theme: Theme;
  setTheme: (theme: Theme) => void;
  resolved: "light" | "dark";
}>(null!);
function savedTheme(): Theme {
  try {
    const value = localStorage.getItem(key);
    if (value === "light" || value === "dark") return value;
  } catch {
    /* Storage may be unavailable in private browsers. */
  }
  return "system";
}
export function ThemeProvider({ children }: { children: ReactNode }) {
  const [theme, setTheme] = useState<Theme>(savedTheme);
  const [systemDark, setSystemDark] = useState(
    () => matchMedia("(prefers-color-scheme: dark)").matches,
  );
  const resolved = theme === "system" ? (systemDark ? "dark" : "light") : theme;
  useEffect(() => {
    const media = matchMedia("(prefers-color-scheme: dark)");
    const change = () => setSystemDark(media.matches);
    media.addEventListener("change", change);
    return () => media.removeEventListener("change", change);
  }, []);
  useEffect(() => {
    document.documentElement.dataset.theme = resolved;
    document.documentElement.style.colorScheme = resolved;
    document
      .querySelector('meta[name="theme-color"]')
      ?.setAttribute("content", resolved === "dark" ? "#141414" : "#fafafa");
    try {
      localStorage.setItem(key, theme);
    } catch {
      /* Theme still works without persistence. */
    }
  }, [theme, resolved]);
  useEffect(() => {
    const listener = (event: StorageEvent) => {
      if (event.key === key) setTheme(savedTheme());
    };
    window.addEventListener("storage", listener);
    return () => window.removeEventListener("storage", listener);
  }, []);
  return (
    <ThemeContext.Provider value={{ theme, setTheme, resolved }}>
      {children}
    </ThemeContext.Provider>
  );
}
export function ThemeControl({ compact = false }: { compact?: boolean }) {
  const { theme, setTheme } = useContext(ThemeContext);
  const Icon = theme === "system" ? Monitor : theme === "dark" ? Moon : Sun;
  return (
    <Select
      className={"theme-control " + (compact ? "compact" : "")}
      icon={<Icon size={16} />}
      placement="auto"
      aria-label="外观主题"
      value={theme}
      onChange={(e) => setTheme(e.target.value as Theme)}
    >
      <option value="system">跟随系统</option>
      <option value="light">浅色模式</option>
      <option value="dark">暗色模式</option>
    </Select>
  );
}
