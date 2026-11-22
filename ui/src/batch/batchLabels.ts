import type { BatchStage, BatchStatus } from "./engine";

export const batchStageLabels: Record<BatchStage, string> = {
  queued: "排队中",
  searching: "搜索中",
  collecting: "采集中",
  choice: "待选择",
  success: "已采集",
  partial: "部分完成",
  not_found: "未找到",
  failed: "失败",
  paused: "已暂停",
  cancelled: "已取消",
};

export const batchLabels: Record<BatchStatus, string> = {
  running: "进行中",
  paused: "已暂停",
  cancelled: "已取消",
};
