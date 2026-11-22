import { useState } from "react";
import { languageText } from "../Catalog";
import { providerLabels, type Candidate } from "../types";

export function CandidateChoice({ candidates, busy, onChoose }: {
  candidates: Candidate[];
  busy: boolean;
  onChoose: (candidate: Candidate) => void;
}) {
  const [selected, setSelected] = useState("");
  const candidate = selected ? candidates[Number(selected)] : undefined;
  return (
    <>
      <div className="mt-2 flex min-w-0 flex-wrap items-center gap-2">
        <label className="filter">
          候选作品
          <select className="field" value={selected} onChange={(event) => setSelected(event.target.value)}>
            <option value="">请选择作品</option>
            {candidates.map((item, index) => (
              <option value={index} key={item.provider + item.id}>
                {[item.title, item.year, languageText(item.original_languages), providerLabels[item.provider]].filter(Boolean).join(" · ")}
              </option>
            ))}
          </select>
        </label>
        <button className="secondary" type="button" disabled={!candidate || busy}
          onClick={() => { if (candidate) onChoose(candidate); }}>采集所选作品</button>
      </div>
    </>
  );
}
