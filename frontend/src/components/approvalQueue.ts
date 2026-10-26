/**
 * Pending approval queue — โมดูลกลางสำหรับ "คำขออนุญาต" ที่ engine ส่งมา
 *
 * ทำไมต้องมี store แยก: เดิมคำขออนุมัติถูกจัดการใน `ApprovalModal`
 * (modal เต็มจอ) ที่ mount อยู่ระดับ App ทำให้ (ก) บังหน้าแชท (ข) ผู้ใช้
 * ที่เข้าจาก Cloudflare Tunnel เห็น modal ไม่ครบ. ตอนนี้คำขอจะถูกวาดเป็น
 * การ์ดในแชท (เหนือช่องพิมพ์) — store นี้ทำให้ทั้งการ์ดในแชทและ "pill"
 * เตือนตอนอยู่แท็บอื่น อ่านคิวเดียวกันได้โดยไม่ต้องมี modal
 *
 * module-level subscription: ผูก IPC ครั้งเดียวตอน import แรก
 * (ไม่ผูกกับ lifecycle ของ component ใด ๆ → ไม่มีคำขอตกหล่นตอนสลับแท็บ)
 */
import { useEffect, useState } from "react";
import { send, subscribe } from "../hooks/useIPC";

export type AgentOrigin =
  | { kind: "main" }
  | { kind: "side_channel"; id: string; agent_name: string }
  | { kind: "subagent"; agent_name: string; depth: number };

export type PendingApproval = {
  id: number;
  tool_name: string;
  input: unknown;
  summary: string | null;
  originator: AgentOrigin;
};

export type ApprovalDecision = "allow" | "allow_for_session" | "deny";

let queue: PendingApproval[] = [];
const listeners = new Set<() => void>();
let wired = false;

function emit() {
  for (const fn of listeners) fn();
}

function wire() {
  if (wired) return;
  wired = true;
  subscribe((msg) => {
    if (msg.type !== "approval_request" || typeof msg.id !== "number") return;
    const id = msg.id;
    // engine re-sends every ~1s until answered — กันคิวบวม
    if (queue.some((r) => r.id === id)) return;
    queue = [
      ...queue,
      {
        id,
        tool_name: (msg.tool_name as string) ?? "?",
        input: msg.input,
        summary: (msg.summary as string | null) ?? null,
        originator: (msg.originator as AgentOrigin) ?? { kind: "main" },
      },
    ];
    emit();
  });
}

export function pendingApprovals(): PendingApproval[] {
  wire();
  return queue;
}

export function subscribeApprovals(fn: () => void): () => void {
  wire();
  listeners.add(fn);
  return () => {
    listeners.delete(fn);
  };
}

/** ตอบกลับ engine แล้วเอาออกจากคิว */
export function respondApproval(id: number, decision: ApprovalDecision) {
  send({ type: "approval_response", id, decision });
  queue = queue.filter((r) => r.id !== id);
  emit();
}

function formatValue(value: unknown): string {
  if (value === null || value === undefined) return "";
  if (typeof value === "string") return value;
  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return String(value);
  }
}

/** สรุป input เป็นบรรทัดอ่านง่าย — คงพฤติกรรมเดิมของ ApprovalModal */
export function summarizeInput(input: unknown): string {
  if (input === null || input === undefined) return "";
  if (typeof input === "string") return input;
  if (typeof input === "object" && !Array.isArray(input)) {
    const entries = Object.entries(input as Record<string, unknown>);
    if (entries.length === 0) return "";
    return entries
      .map(([key, value]) => {
        const rendered = formatValue(value);
        if (rendered.includes("\n")) {
          const indented = rendered
            .split("\n")
            .map((line) => "  " + line)
            .join("\n");
          return `${key}:\n${indented}`;
        }
        return `${key}: ${rendered}`;
      })
      .join("\n");
  }
  try {
    return JSON.stringify(input, null, 2);
  } catch {
    return String(input);
  }
}

/** ให้ component อื่น (เช่น pill บนแท็บอื่น) รู้ว่ามีคำขอค้างอยู่กี่รายการ */
export function usePendingApprovals(): PendingApproval[] {
  const [queue, setQueue] = useState<PendingApproval[]>(pendingApprovals);
  useEffect(() => subscribeApprovals(() => setQueue([...pendingApprovals()])), []);
  return queue;
}
