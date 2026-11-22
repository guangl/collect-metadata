import { useLayoutEffect, useRef, useState } from "react";
import { Movie } from "./Movie";
import type { LookupResponse } from "./types";

export function DetailDialog({ response, opener, closeLabel, onDismiss }: {
  response: LookupResponse;
  opener: HTMLButtonElement;
  closeLabel: string;
  onDismiss: () => void;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const closeButton = useRef<HTMLButtonElement>(null);
  const pressedBackdrop = useRef(false);
  const [notice, setNotice] = useState<{ message: string; error?: boolean } | null>(() => response.storage_error
    ? { message: response.storage_error + " 可以导出本次结果。", error: true } : null);
  const showInfo = (message: string) => setNotice({ message });
  const showError = (message: string) => setNotice({ message, error: true });

  useLayoutEffect(() => {
    const node = dialog.current!;
    node.showModal();
    closeButton.current?.focus({ preventScroll: true });
    return () => {
      node.close();
      if (opener.isConnected) opener.focus({ preventScroll: true });
    };
  }, [opener]);

  return (
    <dialog className="detail-dialog" ref={dialog} aria-label="作品详情" aria-modal="true"
      onCancel={(event) => { event.preventDefault(); onDismiss(); }}
      onPointerDown={(event) => { pressedBackdrop.current = event.target === event.currentTarget; }}
      onClick={(event) => {
        const dismiss = pressedBackdrop.current && event.target === event.currentTarget;
        pressedBackdrop.current = false;
        if (dismiss) onDismiss();
      }}>
      <header className="detail-bar">
        <span>作品详情</span>
        <button className="secondary detail-close" type="button" ref={closeButton}
          aria-label={closeLabel} title={closeLabel} onClick={onDismiss}>
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.75"
            strokeLinecap="round" aria-hidden="true"><path d="m6 6 12 12M18 6 6 18" /></svg>
        </button>
      </header>
      <div className="detail-content">
        {notice && <p className="detail-notice" role={notice.error ? "alert" : "status"} data-kind={notice.error ? "error" : undefined}>{notice.message}</p>}
        <Movie response={response} onNotice={showInfo} onError={showError} />
      </div>
    </dialog>
  );
}
