import { useCallback, useEffect, useState } from "react";
import { getBatchView, listBatches } from "../api";
import { errorText, type StorageStatus } from "../types";
import type { BatchSummary, BatchView } from "./engine";

export function useBatchQueue(active: boolean, storage: StorageStatus, setError: (message: string) => void) {
  const [summaries, setSummaries] = useState<BatchSummary[]>([]);
  const [listLoaded, setListLoaded] = useState(false);
  const [older, setOlder] = useState(false);
  const [selected, setSelected] = useState<number | null>(null);
  const [batch, setBatch] = useState<{ id: number; view: BatchView } | null>(null);
  const [refresh, setRefresh] = useState(0);
  const [pollError, setPollError] = useState<string | null>(null);

  const refreshList = useCallback(async (before: number | null = null) => {
    const batches = await listBatches(before);
    setSummaries((previous) => before === null ? batches : [...previous, ...batches]);
    setOlder(batches.length === 50);
    setSelected((current) => current ?? batches[0]?.id ?? null);
    setListLoaded(true);
  }, []);

  useEffect(() => {
    if (!active || storage !== "ready") return;
    refreshList().catch((error) => setError(errorText(error)));
  }, [active, storage, refreshList, setError]);

  useEffect(() => {
    setPollError(null);
  }, [selected, storage]);

  useEffect(() => {
    if (!active || storage !== "ready" || selected === null) return;
    const id = selected;
    let stopped = false;
    let timer: ReturnType<typeof setTimeout>;
    async function poll() {
      let delay = 1600;
      try {
        const view = await getBatchView(id);
        if (stopped) return;
        setBatch({ id, view });
        setSummaries((previous) => previous.some((item) => item.id === id && item.status !== view.status)
          ? previous.map((item) => item.id === id ? { ...item, status: view.status } : item)
          : previous);
        setPollError(null);
        if (view.rows.some((row) => ["queued", "searching", "collecting"].includes(row.stage))) delay = 650;
      } catch (error) {
        if (!stopped) setPollError(errorText(error));
      } finally {
        if (!stopped) timer = setTimeout(poll, delay);
      }
    }
    void poll();
    return () => {
      stopped = true;
      clearTimeout(timer);
    };
  }, [active, storage, selected, refresh]);

  const view = batch?.id === selected ? batch?.view : undefined;

  function refreshBatch() {
    setRefresh((value) => value + 1);
  }

  return { summaries, listLoaded, older, selected, setSelected, view, pollError, refreshList, refreshBatch };
}
