/**
 * ApprovalCard — คำขออนุญาตเป็น "การ์ดในแชท" พร้อมปุ่มกด
 *
 * แนวคิด: การอนุมัติต้องอยู่ในที่ที่ผู้ใช้เห็นเสมอ (เหนือช่องพิมพ์)
 * ไม่ใช่ modal เต็มจอที่บังบริบทและหายไปเมื่อสลับแท็บ. คิวมาจาก
 * `approvalQueue` (module-level) → หลายคำขอเรียงต่อกันเป็นการ์ดซ้อน
 *
 * ปุ่ม: Deny · Allow for session · Allow (คำเดียวกับ IPC เดิมทุกประการ)
 */
import { useState } from "react";
import { AlertTriangle, Check, ShieldQuestion } from "lucide-react";
import {
  respondApproval,
  summarizeInput,
  usePendingApprovals,
  type PendingApproval,
} from "./approvalQueue";

function originLabel(o: PendingApproval["originator"]): string {
  switch (o.kind) {
    case "main":
      return "main";
    case "side_channel":
      return `${o.agent_name} (background)`;
    case "subagent":
      return `${o.agent_name} (subagent · depth ${o.depth})`;
  }
}

function originColor(o: PendingApproval["originator"]): string {
  switch (o.kind) {
    case "main":
      return "var(--accent)";
    case "side_channel":
      return "#d97706";
    case "subagent":
      return "#2563eb";
  }
}

export function ApprovalCard() {
  const queue = usePendingApprovals();
  const [expanded, setExpanded] = useState(false);

  if (queue.length === 0) return null;
  const current = queue[0];
  const preview = current.summary ?? summarizeInput(current.input);
  const showAllowForSession = current.tool_name !== "MCP server spawn";

  return (
    <div
      className="shrink-0 px-3 py-2"
      style={{
        borderTop: "1px solid var(--border, #2a2a2a)",
        background: "color-mix(in srgb, var(--accent) 8%, var(--bg-secondary))",
      }}
    >
      <div className="flex items-center gap-2 mb-1.5">
        <ShieldQuestion size={14} style={{ color: originColor(current.originator) }} />
        <span style={{ fontSize: "12px", color: "var(--text-primary)" }}>
          ขออนุญาตรัน{" "}
          <code
            className="px-1 py-px rounded"
            style={{
              fontFamily: "Menlo, Monaco, 'Courier New', monospace",
              fontSize: "11px",
              background: "var(--bg-tertiary)",
            }}
          >
            {current.tool_name}
          </code>
          <span style={{ color: "var(--text-secondary)" }}>
            {" "}
            · จาก {originLabel(current.originator)}
          </span>
        </span>
        {queue.length > 1 && (
          <span
            className="px-1.5 py-px rounded"
            style={{
              fontSize: "10px",
              background: "var(--bg-tertiary)",
              color: "var(--text-secondary)",
            }}
          >
            +{queue.length - 1} รอ
          </span>
        )}
        <button
          type="button"
          onClick={() => setExpanded((v) => !v)}
          className="ml-auto text-[11px]"
          style={{ color: "var(--text-secondary)", background: "none", border: "none", cursor: "pointer" }}
        >
          {expanded ? "ซ่อนรายละเอียด" : "ดูรายละเอียด"}
        </button>
      </div>

      {expanded && (
        <pre
          className="mb-2 rounded px-2 py-1.5 overflow-auto"
          style={{
            maxHeight: "28vh",
            background: "var(--bg-primary)",
            color: "var(--text-primary)",
            fontFamily: "Menlo, Monaco, 'Courier New', monospace",
            fontSize: "11px",
            whiteSpace: "pre-wrap",
            wordBreak: "break-all",
            margin: 0,
          }}
        >
          {preview || "(ไม่มีตัวอย่างคำสั่ง)"}
        </pre>
      )}

      <div className="flex items-center gap-2">
        <button
          type="button"
          onClick={() => respondApproval(current.id, "allow")}
          className="inline-flex items-center gap-1 rounded px-3 py-1 text-xs"
          style={{ background: "var(--accent)", color: "var(--accent-fg, #fff)" }}
        >
          <Check size={12} /> อนุญาต
        </button>
        {showAllowForSession && (
          <button
            type="button"
            onClick={() => respondApproval(current.id, "allow_for_session")}
            className="rounded px-3 py-1 text-xs"
            style={{
              background: "transparent",
              border: "1px solid var(--border)",
              color: "var(--text-primary)",
            }}
            title="อนุญาตครั้งนี้และทุกครั้งที่เหลือในเซสชันนี้"
          >
            อนุญาตทั้งเซสชัน
          </button>
        )}
        <button
          type="button"
          onClick={() => respondApproval(current.id, "deny")}
          className="inline-flex items-center gap-1 rounded px-3 py-1 text-xs"
          style={{
            background: "transparent",
            border: "1px solid var(--border)",
            color: "var(--text-secondary)",
          }}
        >
          <AlertTriangle size={12} /> ไม่อนุญาต
        </button>
      </div>
    </div>
  );
}

/** pill เตือนตอนผู้ใช้ไม่ได้อยู่แท็บแชท — ไม่บังหน้าจอ */
export function ApprovalPill({ show = true }: { show?: boolean }) {
  const queue = usePendingApprovals();
  if (!show || queue.length === 0) return null;
  return (
    <div
      className="fixed z-[55] flex items-center gap-2 rounded-full px-3 py-1.5 shadow-lg"
      style={{
        bottom: 16,
        right: 16,
        background: "var(--accent)",
        color: "var(--accent-fg, #fff)",
        fontSize: "12px",
      }}
    >
      <span
        className="activity-ticker-dot inline-block rounded-full"
        style={{ width: 7, height: 7, background: "currentColor" }}
      />
      มีคำขออนุญาต {queue.length} รายการ — เปิดแท็บแชทเพื่อกด
    </div>
  );
}
