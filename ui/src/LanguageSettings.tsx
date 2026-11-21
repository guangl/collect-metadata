import { useEffect, useState } from "react";
import type { FormEvent } from "react";
import { getLanguageSettings, saveLanguages } from "./api";
import { languageName } from "./Catalog";
import { errorText } from "./types";

const choices = ["zh-CN", "zh-TW", "zh-HK", "en", "ja", "ko", "fr", "de", "es", "pt-BR", "it", "ru", "th", "vi"];

export function LanguageSettings() {
  const [languages, setLanguages] = useState<string[] | null>(null);
  const [configured, setConfigured] = useState(false);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<{ text: string; error?: boolean } | null>(null);
  useEffect(() => {
    let active = true;
    getLanguageSettings().then((value) => {
      if (!active) return;
      setLanguages(value.languages);
      setConfigured(value.translation_configured);
    }, (error) => { if (active) setMessage({ text: errorText(error), error: true }); });
    return () => { active = false; };
  }, []);

  async function save(event: FormEvent) {
    event.preventDefault();
    if (busy || !languages) return;
    setBusy(true);
    setMessage(null);
    try {
      const value = await saveLanguages(languages);
      setLanguages(value.languages);
      setConfigured(value.translation_configured);
      setMessage({ text: "已保存，将用于后续采集和同步。" });
    } catch (error) {
      setMessage({ text: errorText(error), error: true });
    } finally {
      setBusy(false);
    }
  }

  return <section className="space-y-3" aria-labelledby="metadata-languages-title">
    <div className="section-heading"><h2 id="metadata-languages-title">元数据语言</h2></div>
    <p className="muted">按选择顺序排列，第一项用于默认显示及 NFO。可多选（最多 8 项），不选则使用简体中文。Emby 插件同步时优先使用 Emby 的元数据语言。</p>
    {languages && <form className="panel batch-form" onSubmit={save}>
      <fieldset className="output-naming" disabled={busy}>
        <legend>目标语言</legend>
        {[...new Set([...choices, ...languages])].map((code) => {
          const index = languages.indexOf(code);
          return <label className="check-option" key={code}>
            <input type="checkbox" checked={index !== -1} disabled={index === -1 && languages.length >= 8}
              onChange={(event) => {
                setLanguages(event.target.checked ? [...languages, code] : languages.filter((value) => value !== code));
                setMessage(null);
              }} />{languageName(code)}{index === 0 && "（首选）"}
          </label>;
        })}
      </fieldset>
      {languages.length > 1 && <label className="batch-input">首选语言
        <select className="field" value={languages[0]} disabled={busy} onChange={(event) => {
          const code = event.target.value;
          setLanguages([code, ...languages.filter((language) => language !== code)]);
          setMessage(null);
        }}>
          {languages.map((code) => <option value={code} key={code}>{languageName(code)}</option>)}
        </select>
      </label>}
      <p className="muted">{configured ? "已配置「元数据翻译」：缺少目标语言时尝试翻译。" : "未配置「元数据翻译」：缺少目标语言时保留原文。"}</p>
      <button type="submit" disabled={busy}>保存语言</button>
    </form>}
    {message && <p role={message.error ? "alert" : "status"}>{message.text}</p>}
  </section>;
}
