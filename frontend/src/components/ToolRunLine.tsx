/**
 * ToolRunLine — รวม tool call ที่ติดกันให้เหลือ "บรรทัดเดียว" ใต้หัวข้อ
 *
 * หลักการ: ในแชทแสดงแค่ "หัวข้อ + ไอคอน" ไม่แสดงคำสั่งยาว ๆ
 * (คำสั่ง/รายละเอียดไปวิ่งอยู่ที่แถบล่าง — ActivityTicker)
 *   - ยังรันอยู่   → `◉ กำลังทำงาน · n รายการ` (จุดกะพริบ)
 *   - เสร็จหมดแล้ว → `✓ n รายการ`
 *
 * คลิกที่บรรทัด = กางดูรายการทั้งหมด (ยังเข้าถึงข้อมูลครบ)
 */
import { useState } from "react";

export type ToolRunItem = {
  /** ป้ายที่แสดงในมุมกาง — ปกติคือ `Bash · <คำสั่ง>` */
  label: string;
  done: boolean;
};

export function ToolRunLine({ items }: { items: ToolRunItem[] }) {
  const [expanded, setExpanded] = useState(false);

  const running = items.filter((it) => !it.done);
  const anyRunning = running.length > 0;

  if (items.length === 0) return null;

  const label = anyRunning
    ? items.length > 1
      ? `กำลังทำงาน · ${running.length}/${items.length} รายการ`
      : "กำลังทำงาน"
    : `${items.length} รายการ`;

  const glyph = anyRunning ? "◉" : "✓";

  return (
    <div className="flex justify-start">
      <div
        className="group flex max-w-[92%] sm:max-w-[80%] flex-col gap-1"
        style={{
          color: "var(--text-secondary)",
          fontFamily: "Menlo, Monaco, 'Courier New', monospace",
          paddingLeft: 2,
          opacity: anyRunning ? 1 : 0.7,
        }}
      >
        <button
          type="button"
          onClick={() => setExpanded((v) => !v)}
          className="inline-flex items-center gap-1.5 text-xs text-left"
          title={label}
          style={{ background: "none", border: "none", padding: 0, cursor: "pointer" }}
        >
          <span
            className={anyRunning ? "activity-ticker-dot" : undefined}
            style={{
              flexShrink: 0,
              width: 5,
              height: 5,
              borderRadius: "50%",
              background: anyRunning ? "var(--accent, #5fb3b3)" : "transparent",
              border: anyRunning ? "none" : "1px solid var(--text-secondary)",
              display: "inline-block",
            }}
          />
          <span className="truncate">
            {glyph} {label}
          </span>
          {items.length > 1 && (
            <span style={{ opacity: 0.5, flexShrink: 0 }}>
              {expanded ? "▾" : "▸"}
            </span>
          )}
        </button>

        {expanded && (
          <div
            className="ml-3 flex flex-col gap-0.5"
            style={{ fontSize: "11px", lineHeight: "1.5" }}
          >
            {items.map((it, i) => (
              <span key={i} className="truncate" title={it.label}>
                {it.done ? "✓" : "▸"} {it.label}
              </span>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
