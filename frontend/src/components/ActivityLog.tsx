/**
 * ActivityLog — บันทึกความเคลื่อนไหวของการทำงาน (ปักเหนือช่องพิมพ์)
 *
 * ประวัติ: เดิมมีแค่ "ticker" บรรทัดเดียวที่ **สลับ** ให้เห็นทีละรายการ
 * ทำให้ตามงานไม่ทัน — เห็นทีละคำสั่ง วนซ้ำ และข้อความถูกตัดจนอ่านไม่ออก.
 * ตอนนี้เป็นแผงที่:
 *
 *   - พับอยู่ (ค่าเริ่มต้น) = บรรทัดเดียว: งานล่าสุด + เป้าหมายที่ประกาศไว้
 *   - กดหัวแผง = กางรายการย้อนหลังทั้งหมด (คำสั่ง/path/query เต็ม ไม่ตัด)
 *     พร้อมเวลาที่ผ่านไปของแต่ละขั้น และหัวข้อเป้าหมายเมื่อมันเปลี่ยน
 *   - สถานะพับ/กางจำไว้ใน localStorage (ผู้ใช้ไม่ต้องกดซ้ำทุกเทิร์น)
 *
 * ข้อมูลทั้งหมดมาจาก `messages` ที่ ChatView มีอยู่แล้ว (tool call + input)
 * ไม่ต้องเพิ่ม IPC ใหม่ — มีแค่ `ts` ที่ ChatView ติดตอนรับ `chat_tool_call`.
 */
import { useEffect, useMemo, useRef, useState } from "react";
import { ChevronDown } from "lucide-react";

export type ActivityMessage = {
  role: string;
  content: string;
  toolName?: string;
  toolKind?: string;
  toolInput?: unknown;
  toolDone?: boolean;
  /// เวลาที่เหตุการณ์เกิด (ms) — ChatView ใส่ให้เฉพาะข้อความสด
  ts?: number;
};

const OPEN_KEY = "thclaws.activity-log-open";
const ALL_KEY = "thclaws.activity-log-all";
const COLLAPSED_MAX = 110;
/// จำนวนรายการที่กางให้ดูตั้งแต่แรก — ที่เกินกว่านี้กด "แสดงทั้งหมด" ได้
const MAX_ROWS = 80;

function oneLine(s: string): string {
  return s.replace(/\s+/g, " ").trim();
}

export function clip(s: string, max = COLLAPSED_MAX): string {
  const one = oneLine(s);
  return one.length > max ? `${one.slice(0, max - 1)}…` : one;
}

function firstLine(s: string, max = 160): string {
  const line = s.split("\n").find((l) => l.trim() !== "") ?? "";
  return clip(line, max);
}

function firstString(inp: unknown): string {
  if (typeof inp === "string") return inp;
  if (!inp || typeof inp !== "object") return "";
  const rec = inp as Record<string, unknown>;
  const preferred = [
    "command",
    "cmd",
    "path",
    "file_path",
    "pattern",
    "query",
    "url",
    "name",
    "key",
    "prompt",
    "text",
    "summary",
    "op",
  ];
  for (const k of preferred) {
    const v = rec[k];
    if (typeof v === "string" && v.trim() !== "") return v;
  }
  for (const v of Object.values(rec)) {
    if (typeof v === "string" && v.trim() !== "") return v;
  }
  return "";
}

/** แปลง tool call → `label` (ชื่อ tool) + `detail` (สิ่งที่สั่งจริง, หลายบรรทัดได้) */
export function describeTool(m: ActivityMessage): { label: string; detail: string } {
  const name = m.toolName ?? m.content ?? "";
  const inp = m.toolInput;
  if (typeof inp === "string" && inp.trim() !== "") {
    return { label: name, detail: inp.trim() };
  }
  if (!inp || typeof inp !== "object") return { label: name, detail: "" };
  const rec = inp as Record<string, unknown>;

  // TodoWrite: รายการงานไม่ใช่ "คำสั่ง" — เอาเฉพาะข้อที่กำลังทำ
  if (Array.isArray(rec.todos)) {
    const items = rec.todos as Array<{ content?: unknown; status?: unknown }>;
    const active = items.find((t) => t.status === "in_progress") ?? items[0];
    const text = typeof active?.content === "string" ? active.content : "";
    return { label: name, detail: text };
  }
  // SubmitPlan: หัวข้อขั้นที่กำลังทำ
  if (Array.isArray(rec.steps)) {
    const steps = rec.steps as Array<{ title?: unknown; status?: unknown }>;
    const active = steps.find((s) => s.status === "in_progress") ?? steps[0];
    const title = typeof active?.title === "string" ? active.title : "";
    return { label: name, detail: title || `${steps.length} ขั้น` };
  }

  // Bash: คำสั่งเต็ม (คงขึ้นบรรทัดใหม่ไว้ให้อ่านสคริปต์ยาว ๆ ได้)
  const command =
    typeof rec.command === "string"
      ? rec.command
      : typeof rec.cmd === "string"
        ? rec.cmd
        : "";
  if (command.trim() !== "") return { label: name, detail: command.trim() };

  // Read/Write/Edit: path + ช่วงบรรทัด; Edit แสดงบรรทัดที่เปลี่ยนด้วย
  const file =
    typeof rec.file_path === "string"
      ? rec.file_path
      : typeof rec.path === "string"
        ? rec.path
        : "";
  if (file !== "") {
    const from = typeof rec.offset === "number" ? rec.offset : undefined;
    const lim = typeof rec.limit === "number" ? rec.limit : undefined;
    const span =
      from !== undefined
        ? `:${from}${lim !== undefined ? `-${from + lim - 1}` : ""}`
        : "";
    let detail = `${file}${span}`;
    if (
      typeof rec.old_string === "string" &&
      typeof rec.new_string === "string"
    ) {
      detail += `\n− ${firstLine(rec.old_string)}`;
      detail += `\n+ ${firstLine(rec.new_string)}`;
    }
    return { label: name, detail };
  }

  if (typeof rec.pattern === "string") {
    const base = typeof rec.path === "string" ? `\nใน ${rec.path}` : "";
    return { label: name, detail: `${rec.pattern}${base}` };
  }
  if (typeof rec.url === "string") return { label: name, detail: rec.url };
  if (typeof rec.query === "string") return { label: name, detail: rec.query };
  // Task/subagent: เอา description/prompt บรรทัดแรก
  if (typeof rec.prompt === "string") {
    return { label: name, detail: firstLine(rec.prompt) };
  }

  // Lesson / WorkLog / Memory: op + key/summary คือตัวระบุ
  if (typeof rec.op === "string") {
    const detail =
      typeof rec.key === "string"
        ? rec.key
        : typeof rec.summary === "string"
          ? rec.summary
          : typeof rec.focus === "string"
            ? rec.focus
            : firstString(rec);
    return { label: `${name} ${rec.op}`, detail };
  }

  return { label: name, detail: firstString(rec) };
}

type Action = {
  id: number;
  label: string;
  detail: string;
  done: boolean;
  /// เป้าหมายที่ agent ประกาศไว้ก่อนเรียก tool นี้ (บรรทัดสถานะล่าสุด)
  goal: string;
  /// วินาทีจากจุดเริ่มเทิร์น (tool แรกหลังข้อความผู้ใช้)
  offset?: number;
};

/** บรรทัดสถานะ = ข้อความ assistant สั้น บรรทัดเดียว ไม่มีโครงสร้าง markdown */
function isGoalLine(text: string): boolean {
  const t = text.trim();
  if (t === "" || t.length > 120 || t.includes("\n")) return false;
  if (/^[#>*|`-]|^\d+\.|\*\*|\[.*\]\(/.test(t)) return false;
  return true;
}

function buildActions(messages: ActivityMessage[]): Action[] {
  const out: Action[] = [];
  let goal = "";
  let turnStart: number | undefined;
  for (let i = 0; i < messages.length; i++) {
    const m = messages[i];
    if (m.role === "user") {
      // เทิร์นใหม่ — เป้าหมายและจุดเริ่มเวลาเริ่มนับใหม่
      goal = "";
      turnStart = undefined;
      continue;
    }
    if (m.role === "assistant") {
      if (isGoalLine(m.content ?? "")) goal = m.content.trim();
      continue;
    }
    if (m.role === "activity") {
      // บรรทัดความเคลื่อนไหวของ subagent ที่รันพื้นหลัง — จบแล้วทันที
      // ไม่มีคู่ result ให้รอ จึงเป็น done เสมอ
      if (turnStart === undefined) turnStart = m.ts;
      out.push({
        id: i,
        label: "",
        detail: (m.content ?? "").trim(),
        done: true,
        goal,
        offset:
          m.ts !== undefined && turnStart !== undefined
            ? Math.max(0, Math.round((m.ts - turnStart) / 1000))
            : undefined,
      });
      continue;
    }
    if (m.role !== "tool") continue;
    const { label, detail } = describeTool(m);
    if (turnStart === undefined) turnStart = m.ts;
    out.push({
      id: i,
      label,
      detail,
      done: m.toolDone === true,
      goal,
      offset:
        m.ts !== undefined && turnStart !== undefined
          ? Math.max(0, Math.round((m.ts - turnStart) / 1000))
          : undefined,
    });
  }
  return out;
}

function fmtOffset(sec: number | undefined): string {
  if (sec === undefined) return "";
  const m = Math.floor(sec / 60);
  const s = sec % 60;
  return `${m}:${String(s).padStart(2, "0")}`;
}

export function ActivityLog({
  messages,
  streaming,
}: {
  messages: ActivityMessage[];
  streaming: boolean;
}) {
  const actions = useMemo(() => buildActions(messages), [messages]);

  const [all, setAll] = useState(() => {
    try {
      return localStorage.getItem(ALL_KEY) === "1";
    } catch {
      return false;
    }
  });
  const hidden = all ? 0 : Math.max(0, actions.length - MAX_ROWS);
  const shown = hidden > 0 ? actions.slice(-MAX_ROWS) : actions;

  const [open, setOpen] = useState(() => {
    try {
      return localStorage.getItem(OPEN_KEY) === "1";
    } catch {
      return false;
    }
  });
  const toggle = () =>
    setOpen((v) => {
      const next = !v;
      try {
        localStorage.setItem(OPEN_KEY, next ? "1" : "0");
      } catch {
        // private mode / storage disabled — ใช้ค่าชั่วคราวในหน่วยความจำ
      }
      return next;
    });
  const showAll = () => {
    setAll(true);
    try {
      localStorage.setItem(ALL_KEY, "1");
    } catch {
      // ignore
    }
  };

  // รายการใหม่เข้ามาแล้วกางอยู่ → เลื่อนลงล่างสุดให้เห็นของสด
  const bodyRef = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    const el = bodyRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [actions.length, open]);

  if (shown.length === 0) return null;

  const running = shown.filter((a) => !a.done);
  const current = running[running.length - 1] ?? shown[shown.length - 1];
  const active = running.length > 0 || (streaming && !current.done);
  const goalLine = current.goal !== "" ? current.goal : "";

  return (
    <div
      className="shrink-0"
      style={{
        borderTop: "1px solid var(--border, #2a2a2a)",
        background: "var(--surface-1, rgba(255,255,255,0.03))",
        fontFamily: "Menlo, Monaco, 'Courier New', monospace",
        fontSize: "11px",
        lineHeight: "16px",
        color: "var(--text-secondary)",
      }}
    >
      <button
        type="button"
        onClick={toggle}
        title={open ? "ย่อบันทึกการทำงาน" : "กางบันทึกการทำงานทั้งหมด"}
        className="flex items-center gap-2 w-full text-left"
        style={{
          padding: "3px 10px",
          minHeight: 23,
          background: "none",
          border: "none",
          cursor: "pointer",
          color: "var(--text-secondary)",
        }}
      >
        <span
          className={active ? "activity-live-dot" : undefined}
          style={{
            flexShrink: 0,
            width: 6,
            height: 6,
            borderRadius: "50%",
            background: active ? "var(--accent, #5fb3b3)" : "var(--text-secondary)",
            opacity: active ? 1 : 0.5,
          }}
        />
        <span style={{ flexShrink: 0, opacity: 0.6, fontSize: "10px" }}>
          {running.length > 0 ? "กำลังทำ" : "ทำแล้ว"} {actions.length - running.length}/
          {actions.length}
          {hidden > 0 ? ` · เห็น ${shown.length}` : ""}
        </span>
        <span
          className="truncate flex-1"
          style={{ whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}
        >
          <span style={{ color: "var(--text-primary)" }}>
            {current.done ? "✓" : "▸"}
            {current.label !== "" ? ` ${current.label}` : ""}
          </span>
          {current.detail !== "" && (
            <> {current.label !== "" ? "›" : ""} {clip(current.detail)}</>
          )}
          {goalLine !== "" && (
            <span style={{ opacity: 0.6 }}> · {clip(goalLine, 70)}</span>
          )}
        </span>
        <ChevronDown
          size={12}
          style={{
            flexShrink: 0,
            opacity: 0.6,
            transform: open ? "rotate(180deg)" : undefined,
          }}
        />
      </button>

      {open && (
        <div
          ref={bodyRef}
          style={{
            maxHeight: 260,
            overflowY: "auto",
            borderTop: "1px solid var(--border, #2a2a2a)",
            padding: "2px 0 4px",
          }}
        >
          {hidden > 0 && (
            <button
              type="button"
              onClick={showAll}
              title="แสดงรายการย้อนหลังทั้งหมดของเทิร์นนี้"
              style={{
                display: "block",
                width: "100%",
                textAlign: "left",
                padding: "3px 10px",
                marginBottom: 2,
                background: "none",
                border: "none",
                borderBottom: "1px dashed var(--border, #2a2a2a)",
                color: "var(--accent, #5fb3b3)",
                cursor: "pointer",
                font: "inherit",
                fontSize: "10px",
                opacity: 0.85,
              }}
            >
              ↑ แสดงย้อนหลังทั้งหมด (+{hidden} รายการ)
            </button>
          )}
          {shown.map((a, i) => {
            const goalChanged =
              a.goal !== "" && (i === 0 || shown[i - 1].goal !== a.goal);
            return (
              <div key={a.id} style={{ padding: "2px 10px" }}>
                {goalChanged && (
                  <div
                    style={{
                      fontSize: "10px",
                      opacity: 0.65,
                      paddingTop: i === 0 ? 2 : 6,
                      whiteSpace: "nowrap",
                      overflow: "hidden",
                      textOverflow: "ellipsis",
                    }}
                    title={a.goal}
                  >
                    🎯 {clip(a.goal, 120)}
                  </div>
                )}
                <div className="flex items-start gap-1.5">
                  <span
                    style={{
                      width: 32,
                      flexShrink: 0,
                      textAlign: "right",
                      opacity: 0.45,
                      fontSize: "10px",
                    }}
                  >
                    {fmtOffset(a.offset)}
                  </span>
                  <span
                    style={{
                      flexShrink: 0,
                      color: a.done
                        ? "var(--success, #6cc070)"
                        : "var(--accent, #5fb3b3)",
                    }}
                  >
                    {a.done ? "✓" : "▸"}
                  </span>
                  {a.label !== "" && (
                    <span
                      style={{
                        flexShrink: 0,
                        color: "var(--text-primary)",
                        whiteSpace: "nowrap",
                      }}
                    >
                      {a.label}
                    </span>
                  )}
                  {a.detail !== "" && (
                    <span
                      style={{
                        whiteSpace: "pre-wrap",
                        wordBreak: "break-word",
                        opacity: 0.85,
                      }}
                    >
                      {a.detail}
                    </span>
                  )}
                </div>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
