import { Check, ChevronDown } from "lucide-react";
import type {
  ChangeEvent,
  KeyboardEvent,
  ReactNode,
  SelectHTMLAttributes,
} from "react";
import {
  Children,
  isValidElement,
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import { createPortal } from "react-dom";
import { nonCredentialAutofill } from "./formAutofill";

type Option = {
  value: string;
  label: ReactNode;
  text: string;
  disabled: boolean;
  group?: string;
};
type Props = SelectHTMLAttributes<HTMLSelectElement> & {
  editable?: { text: string; onTextChange: (text: string) => void; placeholder?: string; validate?: (text: string) => string };
  pending?: boolean;
  icon?: ReactNode;
  placement?: "down" | "auto";
  openFromStart?: boolean;
  renderOption?: (value: string) => ReactNode;
  renderValue?: (value: string) => ReactNode;
  menuClassName?: string;
  menuMinWidth?: number;
  menuMaxWidth?: number;
  menuMaxHeight?: number;
};
function plain(node: ReactNode): string {
  if (typeof node === "string" || typeof node === "number") return String(node);
  if (Array.isArray(node)) return node.map(plain).join("");
  return isValidElement<{ children?: ReactNode }>(node)
    ? plain(node.props.children)
    : "";
}
function optionsOf(children: ReactNode, group?: string): Option[] {
  const result: Option[] = [];
  Children.forEach(children, (child) => {
    if (
      !isValidElement<{
        value?: string | number;
        children?: ReactNode;
        label?: string;
        disabled?: boolean;
      }>(child)
    )
      return;
    if (child.type === "optgroup")
      result.push(...optionsOf(child.props.children, child.props.label));
    else if (child.type === "option")
      result.push({
        value: String(child.props.value ?? plain(child.props.children)),
        label: child.props.children,
        text: plain(child.props.children),
        disabled: !!child.props.disabled,
        group,
      });
  });
  return result;
}
export default function Select({
  children,
  value,
  defaultValue,
  onChange,
  className = "",
  icon,
  placement = "auto",
  openFromStart = false,
  renderOption,
  renderValue,
  menuClassName = "",
  menuMinWidth = 0,
  menuMaxWidth = Number.POSITIVE_INFINITY,
  menuMaxHeight = 320,
  id,
  required,
  disabled: unavailable,
  pending = false,
  name,
  editable,
  ...props
}: Props) {
  const disabled = unavailable || pending;
  const generated = useId();
  const listId = generated + "-list";
  const root = useRef<HTMLDivElement>(null),
    trigger = useRef<HTMLButtonElement | HTMLInputElement>(null),
    menu = useRef<HTMLDivElement>(null),
    native = useRef<HTMLSelectElement>(null);
  const [open, setOpen] = useState(false),
    [active, setActive] = useState(0),
    [internal, setInternal] = useState(String(defaultValue ?? ""));
  const [position, setPosition] = useState<{
    left: number;
    top?: number;
    bottom?: number;
    width: number;
    maxHeight: number;
  }>({
    left: 0,
    top: 0,
    width: 0,
    maxHeight: 300,
  });
  const search = useRef({ text: "", at: 0 });
  const options = optionsOf(children);
  const current = String(value ?? internal);
  const selected = options.find((o) => o.value === current) || options[0];
  const available = options
    .map((o, index) => ({ ...o, index }))
    .filter((o) => !o.disabled);
  const change = (option: Option) => {
    if (option.disabled || disabled) return;
    setInternal(option.value);
    if (native.current) {
      native.current.value = option.value;
      onChange?.({
        target: native.current,
        currentTarget: native.current,
      } as ChangeEvent<HTMLSelectElement>);
    }
    setOpen(false);
    (trigger.current as HTMLInputElement)?.setCustomValidity?.("");
    trigger.current?.focus({ preventScroll: true });
  };
  function show(initial?: number) {
    if (disabled) return;
    const selectedIndex = options.findIndex(
      (o) => o.value === current && !o.disabled,
    );
    setActive(
      initial ??
        (openFromStart
          ? available[0]?.index || 0
          : selectedIndex >= 0
            ? selectedIndex
            : available[0]?.index || 0),
    );
    setOpen(true);
  }
  useLayoutEffect(() => {
    if (!open) return;
    const update = () => {
      const rect = trigger.current?.getBoundingClientRect();
      if (!rect) return;
      if (rect.bottom <= 0 || rect.top >= innerHeight) {
        setOpen(false);
        return;
      }
      const below = Math.max(0, innerHeight - rect.bottom - 12),
        above = Math.max(0, rect.top - 12);
      const height = Math.min(
        menuMaxHeight,
        Math.max(below, above),
        options.length * 40 +
          16 +
          new Set(options.map((o) => o.group).filter(Boolean)).size * 28,
      );
      const upward = placement === "auto" && below < height && above > below;
      const viewportWidth = document.documentElement.clientWidth || innerWidth;
      const width = Math.min(
        Math.max(rect.width, menuMinWidth),
        menuMaxWidth,
        viewportWidth - 16,
      );
      setPosition({
        left: Math.max(8, Math.min(rect.left, viewportWidth - width - 8)),
        top: upward ? undefined : rect.bottom + 5,
        bottom: upward ? innerHeight - rect.top + 5 : undefined,
        width,
        maxHeight: upward ? Math.min(height, above) : Math.min(height, below),
      });
    };
    update();
    window.addEventListener("resize", update);
    const onScroll = (event: Event) => {
      if (!menu.current?.contains(event.target as Node)) update();
    };
    document.addEventListener("scroll", onScroll, true);
    return () => {
      window.removeEventListener("resize", update);
      document.removeEventListener("scroll", onScroll, true);
    };
  }, [open, options.length, placement, menuMinWidth, menuMaxWidth, menuMaxHeight]);
  useEffect(() => {
    if (!open) return;
    const outside = (event: PointerEvent) => {
      if (
        !root.current?.contains(event.target as Node) &&
        !menu.current?.contains(event.target as Node)
      )
        setOpen(false);
    };
    document.addEventListener("pointerdown", outside);
    return () => document.removeEventListener("pointerdown", outside);
  }, [open]);
  useLayoutEffect(() => {
    if (!open || !position.width) return;
    const option = menu.current?.querySelector<HTMLElement>(
      '[data-active="true"]',
    );
    if (option && menu.current) {
      const top = option.offsetTop,
        bottom = top + option.offsetHeight;
      if (active === 0) menu.current.scrollTop = 0;
      else if (top < menu.current.scrollTop) menu.current.scrollTop = top;
      else if (bottom > menu.current.scrollTop + menu.current.clientHeight)
        menu.current.scrollTop = bottom - menu.current.clientHeight;
    }
  }, [open, active, position.width, position.maxHeight]);
  function keyboard(event: KeyboardEvent<HTMLElement>) {
    if (event.key === "Escape" && open) {
      event.preventDefault();
      event.stopPropagation();
      setOpen(false);
      return;
    }
    if (event.key === "Tab") {
      setOpen(false);
      return;
    }
    if (editable && ([" ", "Home", "End"].includes(event.key) || (event.key === "Enter" && !open))) return;
    if (
      ["ArrowDown", "ArrowUp", "Home", "End", "Enter", " "].includes(event.key)
    ) {
      event.preventDefault();
      if (!open) {
        show(
          event.key === "Home"
            ? available[0]?.index
            : event.key === "End"
              ? available.at(-1)?.index
              : undefined,
        );
        return;
      }
      if (event.key === "Enter" || event.key === " ") {
        if (options[active]) change(options[active]);
        return;
      }
      const index = available.findIndex((o) => o.index === active);
      const next =
        event.key === "Home"
          ? 0
          : event.key === "End"
            ? available.length - 1
            : Math.max(
                0,
                Math.min(
                  available.length - 1,
                  index + (event.key === "ArrowDown" ? 1 : -1),
                ),
              );
      setActive(available[next]?.index || 0);
    } else if (!editable && event.key.length === 1) {
      const now = Date.now();
      search.current = {
        text:
          (now - search.current.at < 600 ? search.current.text : "") +
          event.key.toLowerCase(),
        at: now,
      };
      const option = available.find((o) =>
        o.text.toLowerCase().startsWith(search.current.text),
      );
      if (option) {
        if (!open) setOpen(true);
        setActive(option.index);
      }
    }
  }
  let priorGroup: string | undefined;
  return (
    <div ref={root} className={"custom-select " + className}>
      {editable ? <input
        ref={(node) => { trigger.current = node; }}
        id={id} className="select-trigger" role="combobox" type="text" inputMode="numeric"
        {...nonCredentialAutofill}
        value={editable.text} placeholder={editable.placeholder} disabled={disabled} required={required}
        aria-label={props["aria-label"]} aria-labelledby={props["aria-labelledby"]} aria-describedby={props["aria-describedby"]}
        aria-autocomplete="none" aria-expanded={open} aria-controls={open ? listId : undefined}
        aria-activedescendant={open ? listId + "-" + active : undefined} title={props.title}
        onFocus={() => show()} onClick={() => { if (!open) show(); }}
        onKeyDown={keyboard}
        onChange={(e) => { const text = e.target.value; e.currentTarget.setCustomValidity(editable.validate?.(text) || ""); editable.onTextChange(text); setOpen(false); }}
      /> : <button
        ref={(node) => { trigger.current = node; }}
        id={id}
        type="button"
        className="select-trigger"
        role="combobox"
        aria-label={props["aria-label"]}
        aria-labelledby={props["aria-labelledby"]}
        aria-describedby={props["aria-describedby"]}
        aria-expanded={open}
        aria-haspopup="listbox"
        aria-controls={open ? listId : undefined}
        aria-activedescendant={open ? listId + "-" + active : undefined}
        aria-required={required}
        disabled={disabled}
        data-pending={(pending && !unavailable) || undefined}
        title={props.title || selected?.text}
        onClick={() => (open ? setOpen(false) : show())}
        onKeyDown={keyboard}
      >
        {icon && <span className="select-icon">{icon}</span>}
        <span className="select-value">
          {(selected && renderValue?.(selected.value)) ||
            selected?.label ||
            "请选择"}
        </span>
        <ChevronDown size={14} className="select-arrow" />
      </button>}
      <select
        {...nonCredentialAutofill}
        ref={native}
        value={current}
        name={name}
        required={required}
        disabled={disabled}
        aria-hidden="true"
        tabIndex={-1}
        className="select-native"
        style={{ display: "none" }}
        onChange={onChange}
        onInvalid={(e) => {
          e.preventDefault();
          show();
          trigger.current?.focus();
        }}
      >
        {children}
      </select>
      {open &&
        createPortal(
          <div
            ref={menu}
            id={listId}
            role="listbox"
            aria-label={props["aria-label"] || "选项"}
            className={"select-menu " + menuClassName}
            style={{
              ...position,
              visibility: position.width ? "visible" : "hidden",
            }}
          >
            {options.map((option, index) => {
              const heading = option.group !== priorGroup && option.group;
              priorGroup = option.group;
              return (
                <div key={option.value + index}>
                  {heading && <div className="select-group">{heading}</div>}
                  <div
                    id={listId + "-" + index}
                    role="option"
                    aria-selected={option.value === current}
                    aria-disabled={option.disabled || undefined}
                    data-active={index === active}
                    className={
                      "select-option " + (option.disabled ? "disabled" : "")
                    }
                    onMouseDown={(e) => e.preventDefault()}
                    onClick={() => change(option)}
                  >
                    <span>{renderOption?.(option.value) || option.label}</span>
                    {option.value === current && <Check size={14} />}
                  </div>
                </div>
              );
            })}
          </div>,
          root.current?.closest('[role="dialog"], dialog') || document.body,
        )}
    </div>
  );
}
