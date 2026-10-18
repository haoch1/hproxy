import { Children, cloneElement, useEffect, useRef, useId, useState } from "react";
import type { ReactNode, ReactElement, FormEvent, InputHTMLAttributes, ButtonHTMLAttributes } from "react";
import { X, Inbox, ChevronRight, Eye, EyeOff } from "lucide-react";
import { nonCredentialAutofill } from "./formAutofill";

export function Modal({
  title,
  children,
  onClose,
  wide = false,
  fixed = false,
  size = "standard",
}: {
  title: string;
  children: ReactNode;
  onClose: () => void;
  wide?: boolean;
  fixed?: boolean;
  size?: "compact" | "standard" | "details" | "add-server" | "server-settings" | "resource-editor" | "config-viewer" | "update-confirm";
}) {
  const ref = useRef<HTMLDialogElement>(null);
  const titleId = useId();
  useEffect(() => {
    const dialog = ref.current!;
    dialog.showModal();
    dialog.querySelector<HTMLElement>('input:not([type="checkbox"]):not([type="hidden"]), [role="combobox"]')?.focus({ preventScroll: true });
    return () => dialog.close();
  }, []);
  return (
    <dialog
      ref={ref}
      aria-labelledby={titleId}
      className={`modal modal-${size}${wide ? " wide" : ""}${fixed ? " fixed-modal" : ""}`}
      onCancel={(e) => e.preventDefault()}
    >
      <div className="modal-head">
        <h2 id={titleId}>{title}</h2>
        <button className="icon-button" aria-label="关闭" onClick={onClose}>
          <X />
        </button>
      </div>
      <div className="modal-content">{children}</div>
    </dialog>
  );
}
export function Empty({
  title,
  description,
  action,
}: {
  title: string;
  description?: string;
  action?: ReactNode;
}) {
  return (
    <div className="empty">
      <Inbox />
      <strong>{title}</strong>
      {description && <p>{description}</p>}
      {action}
    </div>
  );
}
export function SecretInput({ label, ...props }: InputHTMLAttributes<HTMLInputElement> & { label: string }) {
  const [visible, setVisible] = useState(false);
  return (
    <div className="secret-input">
      <input {...props} {...nonCredentialAutofill} type={visible ? "text" : "password"} spellCheck={false} />
      <button type="button" className="secret-toggle"
        aria-label={`${visible ? "隐藏" : "显示"}${label}`}
        aria-pressed={visible} title={visible ? "隐藏" : "显示"}
        disabled={props.disabled} onClick={() => setVisible((value) => !value)}>
        {visible ? <EyeOff aria-hidden="true" /> : <Eye aria-hidden="true" />}
      </button>
    </div>
  );
}
export function ErrorText({ error }: { error: string }) {
  return error ? (
    <div role="alert" className="error-box">
      {error}
    </div>
  ) : null;
}
/** Lock requests immediately without inserting content or moving neighbouring controls. */
export function PendingButton({
  busy = false,
  disabled = false,
  className = "",
  children,
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement> & { busy?: boolean }) {
  return (
    <button {...props} className={`pending-button ${className}`.trim()}
      disabled={disabled || busy} aria-busy={busy || undefined}
      data-locked={(busy && !disabled) || undefined}>
      {children}
    </button>
  );
}
export function Submit({
  busy,
  disabled = false,
  children = "保存",
}: {
  busy: boolean;
  disabled?: boolean;
  children?: ReactNode;
}) {
  return (
    <PendingButton className="primary" type="submit" busy={busy} disabled={disabled}>
      {children}
    </PendingButton>
  );
}
export function Field({
  label,
  children,
  hint,
  help,
  full = false,
}: {
  label: string;
  children: ReactNode;
  hint?: string;
  help?: ReactNode;
  full?: boolean;
}) {
  const fieldId = useId();
  const control = Children.only(children) as ReactElement<{
    id?: string;
    "aria-describedby"?: string;
    "aria-labelledby"?: string;
  }>;
  const id = control.props.id || fieldId;
  return (
    <div className={full ? "field full" : "field"}>
      {help ? <div className="field-label-row"><label id={`${id}-label`} htmlFor={id}>{label}</label>{help}</div>
        : <label id={`${id}-label`} htmlFor={id}>{label}</label>}
      {cloneElement(control, {
        id,
        "aria-labelledby": `${id}-label`,
        "aria-describedby": hint
          ? `${id}-hint`
          : control.props["aria-describedby"],
      })}
      {hint && <small id={`${id}-hint`}>{hint}</small>}
    </div>
  );
}
export function Confirm({
  title,
  description,
  onConfirm,
  onClose,
  danger = false,
  size = "compact",
  confirmLabel = "确认",
}: {
  title: string;
  description: ReactNode;
  onConfirm: () => Promise<void>;
  onClose: () => void;
  danger?: boolean;
  size?: "compact" | "update-confirm";
  confirmLabel?: string;
}) {
  return (
    <Modal title={title} size={size} onClose={onClose}>
      <ConfirmForm
        description={description}
        onConfirm={onConfirm}
        onClose={onClose}
        danger={danger}
        confirmLabel={confirmLabel}
      />
    </Modal>
  );
}
function ConfirmForm({
  description,
  onConfirm,
  onClose,
  danger,
  confirmLabel,
}: {
  description: ReactNode;
  onConfirm: () => Promise<void>;
  onClose: () => void;
  danger: boolean;
  confirmLabel: string;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  async function submit(e: FormEvent) {
    e.preventDefault();
    if (busy) return;
    setBusy(true);
    try {
      await onConfirm();
      onClose();
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  }
  return (
    <form onSubmit={submit}>
      <p className="confirm-description">{description}</p>
      <ErrorText error={error} />
      <div className="form-actions">
        <button type="button" onClick={onClose}>
          取消
        </button>
        <PendingButton
          type="submit"
          className={danger ? "danger" : "primary"}
          busy={busy}
        >
          {confirmLabel}
        </PendingButton>
      </div>
    </form>
  );
}
export function SectionLink({
  children,
  onClick,
}: {
  children: ReactNode;
  onClick: () => void;
}) {
  return (
    <button className="text-button" onClick={onClick}>
      {children}
      <ChevronRight />
    </button>
  );
}
