import { useEffect, useState } from "react";
import guide from "../../docs/TROUBLESHOOTING.md?raw";
import { desktopLogDirectory, isDesktop, openDesktopLogs } from "./api";
import { errorText } from "./types";

function downloadGuide() {
  const url = URL.createObjectURL(new Blob([guide], { type: "text/markdown;charset=utf-8" }));
  const link = document.createElement("a");
  link.href = url;
  link.download = "Collect-Metadata-排查手册.md";
  link.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

export function Diagnostics() {
  const [directory, setDirectory] = useState<string>();
  const [error, setError] = useState<string>();
  useEffect(() => {
    if (!isDesktop) return;
    let active = true;
    desktopLogDirectory().then(
      (path) => { if (active) setDirectory(path); },
      (reason) => { if (active) setError(errorText(reason)); },
    );
    return () => { active = false; };
  }, []);

  return (
    <section aria-labelledby="diagnostics-title">
      <div className="section-heading"><h2 id="diagnostics-title">日志与排查</h2></div>
      <div className="panel batch-form">
        <p>用报错中的诊断编号查找对应的日志。后台任务可按批次编号和条目编号定位。</p>
        {isDesktop ? <>
          <p className="muted">当前日志为 <code>collect-metadata.log</code>。</p>
          {directory && <p className="break-all"><code>{directory}</code></p>}
        </> : <>
          <p className="muted">在部署主机的 Compose 目录查看 Web 日志。直接运行 Web 时，日志输出到标准错误。</p>
          <pre className="overflow-auto"><code>docker compose logs --since 30m</code></pre>
        </>}
        <div className="settings-actions">
          {isDesktop && directory && <button className="secondary" type="button" onClick={() => {
            setError(undefined);
            void openDesktopLogs().catch((reason) => setError(errorText(reason)));
          }}>打开日志目录</button>}
          <button className="secondary" type="button" onClick={downloadGuide}>下载排查手册</button>
        </div>
        {error && <p className="batch-error" role="alert">{error}</p>}
      </div>
    </section>
  );
}
