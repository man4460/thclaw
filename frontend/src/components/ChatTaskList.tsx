/**
 * ChatTaskList — รายการงานที่ปักไว้เหนือช่องพิมพ์ (ในแชท)
 *
 * ทำไมอยู่ตรงนี้: todo/plan เดิมถูกวาดเป็น "การ์ด" ในสตรีมข้อความทุกครั้งที่
 * model เรียก TodoWrite → แชทยาวขึ้นเรื่อย ๆ และซ้ำกับ sidebar. ตอนนี้:
 *   - รายการเดียว ปักนิ่งเหนือช่องพิมพ์ (ไม่เลื่อนหายไปกับประวัติ)
 *   - ข้อที่กำลังทำ → ไอคอนหมุน (Loader2) · เสร็จแล้ว → เครื่องหมายถูก
 *   - รายละเอียดคำสั่งไม่มาอยู่ที่นี่ — ไปสลับแสดงที่แถบล่าง (ActivityTicker)
 *
 * ข้อมูลมาจาก IPC: `chat_todo_update` (TodoWrite) และ `chat_plan_update`
 * (plan mode). ถ้ามีทั้งคู่ ให้ todo มาก่อน เพราะเป็นงานที่ลงมือทำจริง
 */
import { useEffect, useMemo, useState } from "react";
import { CheckCircle2, Circle, Loader2, ListChecks, ChevronDown } from "lucide-react";
import { subscribe } from "../hooks/useIPC";

type TaskStatus = "pending" | "in_progress" | "done" | "failed" | "completed";

/** แถวดิบจาก `chat_todo_update` (ฟิลด์ชื่อ `content`, ไม่ใช่ `text`) */
type RawTodo = {
  id?: unknown;
  content?: unknown;
  status?: unknown;
};

type Task = {
  id: string;
  text: string;
  status: TaskStatus;
};

type PlanLike = {
  steps: Array<{ id: string; title: string; status: TaskStatus }>;
};

const MAX_VISIBLE = 6;

export function ChatTaskList() {
  const [todos, setTodos] = useState<Task[]>([]);
  const [plan, setPlan] = useState<PlanLike | null>(null);
  const [mode, setMode] = useState<string | null>(null);
  const [expanded, setExpanded] = useState(false);
  const [dismissed, setDismissed] = useState(false);

  useEffect(
    () =>
      subscribe((msg) => {
        if (msg.type === "chat_todo_update") {
          const raw = (msg.todos as RawTodo[] | undefined) ?? [];
          const next = raw.map((t) => ({
            id: String(t.id ?? ""),
            text: String(t.content ?? ""),
            status: (t.status as TaskStatus) ?? "pending",
          }));
          setTodos(next);
          if (next.length > 0) setDismissed(false);
        } else if (msg.type === "chat_plan_update") {
          const p = (msg.plan as PlanLike | null) ?? null;
          setPlan(p && Array.isArray(p.steps) ? p : null);
        } else if (msg.type === "chat_permission_mode") {
          if (typeof msg.mode === "string") setMode(msg.mode);
        }
      }),
    [],
  );

  // todo มาก่อน — ถ้าไม่มี ใช้ขั้นของแผน (ยังไม่เสร็จครบ) แทน
  const tasks = useMemo<Task[]>(() => {
    if (todos.length > 0) return todos;
    if (!plan) return [];
    const allDone = plan.steps.every((s) => s.status === "done");
    if (allDone && mode !== "plan") return [];
    return plan.steps.map((s) => ({ id: s.id, text: s.title, status: s.status }));
  }, [todos, plan, mode]);

  if (tasks.length === 0 || dismissed) return null;

  const done = tasks.filter((t) => t.status === "completed" || t.status === "done").length;
  const active = tasks.find((t) => t.status === "in_progress");
  const visible = expanded ? tasks : tasks.slice(0, MAX_VISIBLE);
  const hidden = tasks.length - visible.length;

  return (
    <div
      className="shrink-0"
      style={{
        borderTop: "1px solid var(--border, #2a2a2a)",
        background: "var(--surface-1, rgba(255,255,255,0.03))",
        padding: "4px 10px",
      }}
    >
      <div className="flex items-center gap-2" style={{ fontSize: "11px" }}>
        <ListChecks size={12} style={{ color: "var(--text-secondary)", flexShrink: 0 }} />
        <span style={{ color: "var(--text-secondary)" }}>
          {done}/{tasks.length}
        </span>
        {active && (
          <span className="truncate" style={{ color: "var(--text-primary)" }} title={active.text}>
            · {active.text}
          </span>
        )}
        {(hidden > 0 || expanded) && (
          <button
            type="button"
            onClick={() => setExpanded((v) => !v)}
            className="ml-auto inline-flex items-center gap-1"
            style={{
              background: "none",
              border: "none",
              padding: 0,
              cursor: "pointer",
              color: "var(--text-secondary)",
              fontSize: "10px",
            }}
          >
            {expanded ? "ย่อ" : `+${hidden}`}
            <ChevronDown
              size={11}
              style={{ transform: expanded ? "rotate(180deg)" : "none" }}
            />
          </button>
        )}
        <button
          type="button"
          onClick={() => setDismissed(true)}
          className={hidden > 0 || expanded ? "" : "ml-auto"}
          style={{
            background: "none",
            border: "none",
            padding: 0,
            cursor: "pointer",
            color: "var(--text-secondary)",
            fontSize: "10px",
            opacity: 0.6,
          }}
          title="ซ่อนรายการนี้ (จะกลับมาเมื่อมีการอัปเดต)"
        >
          ✕
        </button>
      </div>

      <div className="mt-0.5 flex flex-col" style={{ gap: 1 }}>
        {visible.map((t) => {
          const finished = t.status === "completed" || t.status === "done";
          const running = t.status === "in_progress";
          return (
            <div
              key={t.id}
              className="flex items-center gap-1.5"
              style={{ fontSize: "11px", lineHeight: "1.5" }}
              title={t.text}
            >
              <span style={{ flexShrink: 0, display: "inline-flex", width: 12 }}>
                {finished ? (
                  <CheckCircle2 size={11} style={{ color: "var(--success, #6cc070)" }} />
                ) : running ? (
                  <Loader2
                    size={11}
                    className="task-spin"
                    style={{ color: "var(--accent, #5fb3b3)" }}
                  />
                ) : (
                  <Circle size={11} style={{ color: "var(--text-secondary)", opacity: 0.6 }} />
                )}
              </span>
              <span
                className="truncate"
                style={{
                  color: finished
                    ? "var(--text-secondary)"
                    : running
                      ? "var(--text-primary)"
                      : "var(--text-secondary)",
                  textDecoration: finished ? "line-through" : "none",
                  opacity: finished ? 0.75 : 1,
                }}
              >
                {t.text}
              </span>
            </div>
          );
        })}
      </div>
    </div>
  );
}
