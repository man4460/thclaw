/**
 * ToolRunLine — รวม tool call ที่ติดกันให้เหลือ "บรรทัดเดียว" ใต้หัวข้อ
 *
 * ทำอะไร: แทนที่จะแสดง tool ทุกตัวเป็นคนละบรรทัด (กำแพง บรรทัด)
 *   กลุ่มที่ติดกันจะยุบเป็น 1 บรรทัด แล้ว **สลับข้อความวิ่ง** ไปเรื่อย ๆ
 *   - ยังมีตัวรันอยู่ → หมุนเวียนแสดงทีละคำสั่ง (▸) ทุก ~1.8 วิ
 *   - เสร็จหมดแล้ว    → หยุดนิ่งที่บรรทัดสรุป `✓ n · <คำสั่งล่าสุด>`
 *
 * คลิกที่บรรทัด = กางดูรายการทั้งหมด (ยังเข้าถึงข้อมูลครบ)
 * ข้อความยาวเกิน = ตัดด้วย … (hover ดูเต็มผ่าน title)
 */
import { useEffect, useState } from "react";

export type ToolRunItem = {
  /** ป้ายที่แสดง — ปกติคือ `Bash · <คำสั่ง>` */
  label: string;
  done: boolean;
};

export function ToolRunLine({ items }: { items: ToolRunItem[] }) {
  const [expanded, setExpanded] = useState(false);
  const [idx, setIdx] = useState(0);

  const running = items.filter((it) => !it.done);
  const anyRunning = running.length > 0;

  // หมุนเวียนเฉพาะตอนยังทำงานอยู่ — เสร็จแล้วปล่อยนิ่ง ไม่กวนสายตา
  useEffect(() => {
    if (!anyRunning || items.length < 2) return;
    const t = window.setInterval(
      () => setIdx((v) => (v + 1) % items.length),
      1800,
    );
    return () => window.clearInterval(t);
  }, [anyRunning, items.length]);

  if (items.length === 0) return null;

  const current = items[idx % items.length];

  const label = anyRunning ? current.label : `${items.length} รายการ`;

  const glyph = anyRunning ? "▸" : "✓";

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
