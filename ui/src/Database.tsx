import { useState } from "react";
import type { FormEvent } from "react";
import { configureDatabase, getStorageStatus, isDesktop } from "./api";
import { errorText, storageLabels } from "./types";
import type { StorageStatus } from "./types";

export function Database({ storage, onStorage, onConnected }: {
  storage: StorageStatus;
  onStorage: (status: StorageStatus) => void;
  onConnected: () => void;
}) {
  const [url, setUrl] = useState("");
  const [action, setAction] = useState<"test" | "save" | null>(null);
  const [message, setMessage] = useState<{ text: string; error?: boolean } | null>(null);

  async function test() {
    if (action) return;
    setAction("test");
    setMessage(null);
    try {
      const next = await getStorageStatus();
      onStorage(next);
      setMessage({ text: next === "ready" ? "PostgreSQL 连接测试成功。" : storageLabels[next] });
    } catch (error) {
      onStorage("failed");
      setMessage({ text: errorText(error), error: true });
    } finally {
      setAction(null);
    }
  }

  async function save(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (action || !isDesktop || !url.trim()) return;
    setAction("save");
    setMessage(null);
    try {
      await configureDatabase(url);
      setUrl("");
      onConnected();
      setMessage({ text: "已测试并保存 PostgreSQL 连接。" });
    } catch (error) {
      setMessage({ text: errorText(error), error: true });
    } finally {
      setAction(null);
    }
  }

  return (
    <section aria-labelledby="database-title">
      <div className="section-heading"><h2 id="database-title">PostgreSQL</h2></div>
      <form className="batch-form settings-form" onSubmit={save}>
        <p className="muted">{storageLabels[storage]}</p>
        {isDesktop ? <>
          <label htmlFor="database-url" className="batch-input">PostgreSQL 连接地址</label>
          <input id="database-url" type="password" autoComplete="off" spellCheck={false}
            placeholder="postgresql://用户名:密码@主机:端口/数据库"
            value={url} onChange={(event) => { setUrl(event.target.value); setMessage(null); }}
            disabled={action !== null} />
        </> : <p className="muted">Web 使用服务端配置的数据库。更换连接请修改 <code>DATABASE_URL</code> 或 <code>PG*</code> 环境变量。</p>}
        <div className="settings-actions">
          <button className="secondary" type="button" disabled={action !== null || storage === "none"}
            onClick={test}>{action === "test" ? "正在测试…" : "测试当前数据库"}</button>
          {isDesktop && <button className="primary" type="submit" disabled={action !== null || !url.trim()}>
            {action === "save" ? "正在连接…" : "测试连接并保存"}
          </button>}
        </div>
        {message && <p className={message.error ? "batch-error" : "muted"}
          role={message.error ? "alert" : "status"}>{message.text}</p>}
      </form>
    </section>
  );
}
