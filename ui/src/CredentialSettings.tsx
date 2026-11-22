import { useEffect, useState } from "react";
import type { FormEvent } from "react";
import { getBangumiStatus, getTmdbStatus, saveBangumi, saveTmdb, testBangumi, testTmdb } from "./api";
import { ExternalLink } from "./Catalog";
import { errorText } from "./types";
import type { CredentialStatus } from "./types";

const providers = {
  tmdb: {
    name: "TMDB",
    empty: "尚未配置 TMDB",
    label: "TMDB API 凭据",
    placeholder: "API Read Access Token 或 v3 API Key",
    link: "https://www.themoviedb.org/settings/api",
    getStatus: getTmdbStatus,
    test: testTmdb,
    save: saveTmdb,
  },
  bangumi: {
    name: "Bangumi",
    empty: "公开条目可直接采集，访问令牌为可选项。",
    label: "Bangumi 访问令牌",
    placeholder: "Access Token",
    link: "https://bgm.tv/dev/app",
    getStatus: getBangumiStatus,
    test: testBangumi,
    save: saveBangumi,
  },
} as const;

export function CredentialSettings({ source, onError }: {
  source: keyof typeof providers;
  onError: (message: string) => void;
}) {
  const provider = providers[source];
  const { name } = provider;
  const labels: Record<CredentialStatus, string> = {
    unconfigured: provider.empty,
    environment: "当前使用环境变量中的凭据",
    saved: `已保存 ${name} 凭据`,
  };
  const [status, setStatus] = useState<CredentialStatus | null>(null);
  const [credential, setCredential] = useState("");
  const [action, setAction] = useState<"test" | "save" | null>(null);
  const [message, setMessage] = useState<{ text: string; error?: boolean } | null>(null);
  const value = credential.trim();
  const hasSavedCredential = status === "environment" || status === "saved";

  useEffect(() => {
    let active = true;
    provider.getStatus().then(
      (status) => { if (active) setStatus((current) => current ?? status); },
      (error) => { if (active) setMessage((current) => current ?? { text: errorText(error), error: true }); },
    );
    return () => { active = false; };
  }, [provider]);

  async function test() {
    if (action || (!value && !hasSavedCredential)) return;
    setAction("test");
    setMessage(null);
    try {
      await provider.test(value || null);
      setMessage({ text: `${name} 连接测试成功。` });
    } catch (error) {
      setMessage({ text: errorText(error), error: true });
    } finally {
      setAction(null);
    }
  }

  async function save(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (action || !value) return;
    setAction("save");
    setMessage(null);
    try {
      setStatus(await provider.save(value));
      setCredential("");
      setMessage({ text: "已验证并保存，电影和剧集采集即刻生效。" });
    } catch (error) {
      setMessage({ text: errorText(error), error: true });
    } finally {
      setAction(null);
    }
  }

  return (
    <section aria-labelledby={source + "-title"}>
      <div className="section-heading"><h2 id={source + "-title"}>{name}</h2></div>
      <form className="panel batch-form" onSubmit={save}>
        {status ? <p className="muted">{labels[status]}</p>
          : !message && <p className="muted" role="status">正在读取 {name} 配置…</p>}
        <label htmlFor={source + "-credential"} className="batch-input">{provider.label}</label>
        <input className="field w-full" id={source + "-credential"} type="password" autoComplete="off" spellCheck={false}
          placeholder={provider.placeholder} maxLength={8192}
          value={credential} onChange={(event) => { setCredential(event.target.value); setMessage(null); }}
          disabled={action !== null} />
        <p className="muted">
          <ExternalLink value={provider.link} onError={onError}>获取 {provider.label} ↗</ExternalLink>
        </p>
        <div className="settings-actions">
          <button type="button" className="secondary" disabled={action !== null || (!value && !hasSavedCredential)}
            onClick={test}>{action === "test" ? "正在测试…" : "测试连接"}</button>
          <button type="submit" className="primary" disabled={action !== null || !value}>
            {action === "save" ? "正在验证并保存…" : "保存配置"}
          </button>
        </div>
        {message && <p className={message.error ? "batch-error" : "muted"}
          role={message.error ? "alert" : "status"}>{message.text}</p>}
      </form>
    </section>
  );
}
