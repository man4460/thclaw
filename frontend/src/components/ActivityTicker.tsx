/**
 * ActivityTicker — แถบสถานะวิ่งที่ขอบล่างของหน้าแชท
 *
 * ทำอะไร: บอก "ตอนนี้ agent กำลังทำอะไร" เป็นบรรทัดเดียว
 *   - มี tool รันอยู่  → `▸ Bash · <คำสั่ง>` (อ่านจาก toolInput.command)
 *   - กำลังพิมพ์ข้อความ → บรรทัดสุดท้ายของข้อความ assistant (สถานะที่ agent ประกาศ)
 *   - ทำทั้งสองพร้อมกันได้ → `สถานะ  ›  ▸ Bash · …`
 *
 * ข้อความยาวเกินพื้นที่ → วิ่งวน (marquee) ต่อเนื่อง ตามที่ผู้ใช้ขอ
 * ("แสดงด้านล่างวิ่งตลอด") ไม่ต้อง hover ไม่ต้องคลิก
 *
 * ไม่แสดงเมื่อไม่มีอะไรเกิดขึ้น — แถบเปล่าจะกินพื้นที่ฟรี
 */
import { useEffect, useMemo, useRef, useState } from "react";

export type TickerMessage = {
  role: string;
  content: string;
  toolName?: string;
  toolKind?: string;
  toolInput?: unknown;
  toolDone?: boolean;
  thinking?: string;
  thinkingChars?: number;
};

/** ดึงคำสั่งที่กำลังรันจาก toolInput — Bash ใช้ command, tool อื่นใช้สรุปสั้น ๆ */
function describeTool(msg: TickerMessage): string {
  const inp = msg.toolInput;
  const name = msg.toolName ?? msg.content;
  if (inp && typeof inp === "object") {
    const rec = inp as Record<string, unknown>;
    if (typeof rec.command === "string" && rec.command.trim() !== "") {
      return `${name} · ${rec.command.replace(/\s+/g, " ").trim()}`;
    }
    if (typeof rec.path === "string") return `${name} · ${rec.path}`;
    if (typeof rec.file_path === "string") return `${name} · ${rec.file_path}`;
    if (typeof rec.pattern === "string") return `${name} · ${rec.pattern}`;
    if (typeof rec.query === "string") return `${name} · ${rec.query}`;
    if (typeof rec.url === "string") return `${name} · ${rec.url}`;
  }
  return name;
}

/** บรรทัดสุดท้ายที่ไม่ว่างของข้อความ assistant — บรรทัดที่ agent ใช้บอกสถานะ */
function lastStatusLine(messages: TickerMessage[]): string {
  for (let i = messages.length - 1; i >= 0; i--) {
    const m = messages[i];
    if (m.role !== "assistant") continue;
    const lines = m.content
      .split("\n")
      .map((s) => s.trim())
      .filter((s) => s !== "");
    if (lines.length > 0) return lines[lines.length - 1];
  }
  return "";
}

export function ActivityTicker({
  messages,
  streaming,
}: {
  messages: TickerMessage[];
  streaming: boolean;
}) {
  const running = useMemo(
    () => messages.filter((m) => m.role === "tool" && !m.toolDone),
    [messages],
  );

  const text = useMemo(() => {
    const status = lastStatusLine(messages);
    const tool =
      running.length > 0 ? describeTool(running[running.length - 1]) : "";
    if (tool && status) return `${status}   ›   ▸ ${tool}`;
    if (tool) return `▸ ${tool}`;
    if (status) return status;
    return "";
  }, [messages, running]);

  // ตรวจว่าข้อความล้นพื้นที่ไหม → ถ้าล้น ให้วิ่ง
  const wrapRef = useRef<HTMLDivElement>(null);
  const measureRef = useRef<HTMLSpanElement>(null);
  const [marquee, setMarquee] = useState<{ on: boolean; duration: number }>({
    on: false,
    duration: 12,
  });

  useEffect(() => {
    const wrap = wrapRef.current;
    const measure = measureRef.current;
    if (!wrap || !measure) return;
    const width = measure.scrollWidth;
    const overflow = width - wrap.clientWidth > 8;
    // ความเร็วคงที่ ~45px/วินาที — ข้อความยาววิ่งนานขึ้น ไม่ใช่เร็วขึ้น
    setMarquee({
      on: overflow,
      duration: Math.max(8, Math.round(width / 45)),
    });
  }, [text]);

  if (text === "") return null;

  return (
    <div
      className="activity-ticker flex items-center gap-2 shrink-0"
      style={{
        padding: "3px 10px",
        background: "var(--surface-1, rgba(255,255,255,0.03))",
        borderTop: "1px solid var(--border, #2a2a2a)",
        fontFamily: "Menlo, Monaco, 'Courier New', monospace",
        fontSize: "11px",
        lineHeight: "16px",
        color: "var(--text-secondary)",
        minHeight: 23,
      }}
      title={text}
    >
      <span
        className={streaming ? "activity-ticker-dot" : undefined}
        style={{
          flexShrink: 0,
          width: 6,
          height: 6,
          borderRadius: "50%",
          background: running.length > 0 ? "var(--accent, #5fb3b3)" : "var(--text-secondary)",
          opacity: running.length > 0 ? 1 : 0.5,
        }}
      />
      <div ref={wrapRef} className="flex-1 overflow-hidden" style={{ whiteSpace: "nowrap" }}>
        {/* ตัววัดความกว้างจริง (ไม่แสดง) — ใช้คำนวณว่าต้องวิ่งหรือไม่ */}
        <span
          ref={measureRef}
          aria-hidden
          style={{ position: "absolute", visibility: "hidden", whiteSpace: "nowrap" }}
        >
          {text}
        </span>
        {marquee.on ? (
          <div
            className="activity-ticker-track"
            style={{ animationDuration: `${marquee.duration}s` }}
          >
            <span style={{ paddingRight: "3rem" }}>{text}</span>
            <span style={{ paddingRight: "3rem" }} aria-hidden>
              {text}
            </span>
          </div>
        ) : (
          <span className="truncate" style={{ display: "block" }}>
            {text}
          </span>
        )}
      </div>
    </div>
  );
}
