/**
 * PlanApprovalCard — ปุ่ม Approve / Cancel แบบ inline ในหน้าแชท
 *
 * ทำไมมี: ปุ่มเดิมอยู่ใน PlanSidebar (sidebar ขวา) ซึ่งบางครั้งผู้ใช้ไม่เห็น
 * (จอแคบ / sidebar ถูก dismiss / snapshot ยังไม่มา) ทำให้แผนค้างรออนุมัติ
 * โดยไม่มีทางไปต่อ การ์ดนี้จึงยึด "ปุ่มต้องอยู่ในที่ที่มองเห็นเสมอ" — วาง
 * เหนือช่องพิมพ์ในแชท แล้วส่ง IPC เดียวกับ sidebar (`plan_approve` /
 * `plan_cancel`) จึงทำงานเหมือนกันทุกประการ
 *
 * แสดงเฉพาะ: mode === "plan" + มีแผนที่ยังไม่ done ครบ
 * หลังกดอนุมัติ mode จะเปลี่ยน → การ์ดหายไปเอง (ไม่ต้องซ่อนด้วย state)
 */
import { useEffect, useState } from "react";
import { Check, X } from "lucide-react";
import { send, subscribe } from "../hooks/useIPC";

type StepStatus = "todo" | "in_progress" | "done" | "failed";

type PlanStep = {
  id: string;
  title: string;
  status: StepStatus;
};

type Plan = {
  id: string;
  steps: PlanStep[];
};

/** จำนวนขั้นที่โชว์เป็นตัวอย่างก่อนกดอนุมัติ — ที่เหลือบอกแค่จำนวน */
const PREVIEW_STEPS = 4;

export function PlanApprovalCard() {
  const [plan, setPlan] = useState<Plan | null>(null);
  const [mode, setMode] = useState<string | null>(null);

  useEffect(
    () =>
      subscribe((msg) => {
        if (msg.type === "chat_plan_update") {
          setPlan((msg.plan as Plan | null) ?? null);
        } else if (msg.type === "chat_permission_mode") {
          if (typeof msg.mode === "string") setMode(msg.mode);
        }
      }),
    [],
  );

  const allDone =
    plan !== null && plan.steps.every((s) => s.status === "done");
  if (mode !== "plan" || !plan || allDone) return null;

  const total = plan.steps.length;
  const preview = plan.steps.slice(0, PREVIEW_STEPS);
  const rest = total - preview.length;

  return (
    <div
      className="shrink-0 px-3 py-2"
      style={{
        borderTop: "1px solid var(--border, #2a2a2a)",
        background: "var(--surface-1, rgba(255,255,255,0.03))",
      }}
    >
      <div className="flex items-center justify-between gap-3 mb-1.5">
        <span style={{ fontSize: "12px", color: "var(--text-primary)" }}>
          แผนพร้อมอนุมัติ · {total} ขั้น
        </span>
        <span style={{ fontSize: "11px", color: "var(--text-secondary)" }}>
          ตรวจรายละเอียดเต็มที่ sidebar ขวา
        </span>
      </div>

      <ol
        className="mb-2"
        style={{
          margin: 0,
          paddingLeft: 18,
          fontSize: "11px",
          lineHeight: "1.6",
          color: "var(--text-secondary)",
        }}
      >
        {preview.map((s) => (
          <li key={s.id} className="truncate" title={s.title}>
            {s.title}
          </li>
        ))}
        {rest > 0 && <li style={{ listStyle: "none", opacity: 0.6 }}>…</li>}
      </ol>

      <div className="flex gap-2">
        <button
          type="button"
          onClick={() => send({ type: "plan_approve" })}
          className="flex items-center gap-1.5 px-3 py-1 rounded text-xs font-medium"
          style={{
            background: "var(--accent)",
            color: "var(--accent-fg, #fff)",
            border: "none",
            cursor: "pointer",
          }}
          title="อนุมัติแผนและเริ่มลงมือ — ปลดล็อกเครื่องมือที่เขียนไฟล์ได้"
        >
          <Check size={12} /> อนุมัติ &amp; เริ่มทำงาน
        </button>
        <button
          type="button"
          onClick={() => send({ type: "plan_cancel" })}
          className="flex items-center gap-1.5 px-3 py-1 rounded text-xs"
          style={{
            background: "transparent",
            color: "var(--text-secondary)",
            border: "1px solid var(--border, #2a2a2a)",
            cursor: "pointer",
          }}
          title="ทิ้งแผนและออกจาก plan mode"
        >
          <X size={12} /> ยกเลิก
        </button>
      </div>
    </div>
  );
}
