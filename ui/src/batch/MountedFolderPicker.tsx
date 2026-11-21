import { useState } from "react";
import { browseMountedFolders } from "../api";
import { errorText } from "../types";

export function MountedFolderPicker({ busy, onSelect }: { busy: boolean; onSelect: (path: string) => void }) {
  const [folder, setFolder] = useState<{ path: string; children: string[] }>();
  const [child, setChild] = useState("");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string>();

  async function browse(path: string) {
    if (loading || busy) return;
    setLoading(true);
    setError(undefined);
    try {
      const children = await browseMountedFolders(path);
      setFolder({ path, children });
      setChild("");
    } catch (error) {
      setError(errorText(error));
    } finally {
      setLoading(false);
    }
  }

  const disabled = busy || loading;
  return <div className="mounted-folders">
    {!folder ? <button className="secondary" type="button" disabled={disabled} onClick={() => browse("")}>
      {loading ? "正在读取挂载目录…" : "浏览服务器挂载目录"}
    </button> : <>
      <p className="folder-path muted">挂载目录：<code>共享目录{folder.path && " / " + folder.path}</code></p>
      <div className="folder-buttons">
        {folder.path && <button className="secondary" type="button" disabled={disabled}
          onClick={() => browse(folder.path.split("/").slice(0, -1).join("/"))}>上一级</button>}
        {folder.children.length > 0 && <>
          <label className="filter">子目录
            <select value={child} onChange={(event) => setChild(event.target.value)} disabled={disabled}>
              <option value="">选择子目录</option>
              {folder.children.map((name) => <option key={name} value={name}>{name}</option>)}
            </select>
          </label>
          <button className="secondary" type="button" disabled={disabled || !child}
            onClick={() => browse(folder.path ? folder.path + "/" + child : child)}>打开子目录</button>
        </>}
        <button className="secondary" type="button" disabled={disabled} onClick={() => onSelect(folder.path)}>
          预览当前挂载目录
        </button>
      </div>
      {loading && <p className="muted" role="status">正在读取挂载目录…</p>}
    </>}
    {error && <p className="batch-error" role="alert">{error}</p>}
  </div>;
}
