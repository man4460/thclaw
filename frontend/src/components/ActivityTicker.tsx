/**
 * ActivityTicker — แถบสถานะวิ่งที่ขอบล่างของหน้าแชท
 *
 * หลักการ: ในแชทไม่ต้องมีบรรทัดคำสั่งยาว ๆ (อันนั้นถูกยุบเป็น
 * "หัวข้อ + ไอคอน" ด้วย ToolRunLine) — รายละเอียดทั้งหมดมาอยู่ที่นี่
 * เป็นบรรทัดเดียวที่ **สลับ** ให้เห็นทีละรายการ:
 *
 *   - มี tool รันหลายตัว → สลับ `▸ Bash · <คำสั่ง>` ของแต่ละตัว (~3 วิ)
 *   - มีสถานะจากข้อความ assistant → สลับคั่นระหว่างสถานะกับคำสั่ง
 *   - ข้อความยาวเกินพื้นที่ → วิ่งวน (marquee) ต่อเนื่อง
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
    // TodoWrite / SubmitPlan: เอาเนื้อหาที่มีความหมาย ไม่ใช่ JSON ดิบ
    if (Array.isArray(rec.todos)) {
      const items = rec.todos as Array<{ content?: unknown; status?: unknown }>;
      const active = items.find((t) => t.status === "in_progress") ?? items[0];
      if (active && typeof active.content === "string") {
        return `${name} · ${active.content.replace(/\s+/g, " ").trim()}`;
      }
    }
    if (Array.isArray(rec.steps)) {
      const steps = rec.steps as Array<{ title?: unknown; status?: unknown }>;
      const active = steps.find((s) => s.status === "in_progress") ?? steps[0];
      if (active && typeof active.title === "string") {
        return `${name} · ${active.title.replace(/\s+/g, " ").trim()}`;
      }
      return `${name} · ${steps.length} ขั้น`;
    }
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

/** ข้อความที่สลับวน — สถานะล่าสุดก่อน แล้วตามด้วยคำสั่งที่รันอยู่ */
function tickerItems(
  messages: TickerMessage[],
  running: TickerMessage[],
): string[] {
  const items: string[] = [];
  const status = lastStatusLine(messages);
  if (status) items.push(status);
  for (const m of running) items.push(`▸ ${describeTool(m)}`);
  return items;
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
  const items = useMemo(
    () => tickerItems(messages, running),
    [messages, running],
  );
  const itemsKey = items.join("\u0000");

  // สลับรายการเมื่อมีมากกว่า 1 — เห็นทั้งสถานะและคำสั่งครบโดยไม่ต้องกวาดตา
  const [idx, setIdx] = useState(0);
  useEffect(() => {
    if (items.length < 2) return;
    const t = window.setInterval(
      () => setIdx((v) => (v + 1) % items.length),
      3000,
    );
    return () => window.clearInterval(t);
    // itemsKey ทำให้ interval รีเซ็ตเมื่อชุดรายการเปลี่ยน (ไม่ใช่ทุก render)
  }, [items.length, itemsKey]);

  // ไม่เก็บ idx ที่ "หมดอายุ" — หารเอาเศษเสมอ (รายการหดตัวก็ไม่ค้าง)
  const shownIdx = items.length === 0 ? 0 : idx % items.length;
  const text = items.length === 0 ? "" : items[shownIdx];

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
        className={streaming || running.length > 0 ? "activity-ticker-dot" : undefined}
        style={{
          flexShrink: 0,
          width: 6,
          height: 6,
          borderRadius: "50%",
          background: running.length > 0 ? "var(--accent, #5fb3b3)" : "var(--text-secondary)",
          opacity: running.length > 0 ? 1 : 0.5,
        }}
      />
      {/* จุดบอกว่ามีรายการให้สลับกี่รายการ — ช่วยให้รู้ว่าไม่ได้ค้าง */}
      {items.length > 1 && (
        <span style={{ flexShrink: 0, opacity: 0.55, fontSize: "10px" }}>
          {shownIdx + 1}/{items.length}
        </span>
      )}
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
