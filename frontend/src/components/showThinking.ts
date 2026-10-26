//! การตั้งค่าการแสดง Reasoning ("Thinking") ในหน้าแชท
//!
//! ผู้ใช้ส่วนใหญ่ไม่ต้องการบล็อก `▾ Thinking (N chars)` เพราะกินที่และ
//! เนื้อหาส่วนใหญ่เป็นภาษาที่โมเดลคิดก่อนตอบ — ค่าเริ่มต้นจึงเป็น "ซ่อน"
//! ตัวเลือกเก็บใน localStorage (เบา ไม่ต้องผ่าน engine) และแชทจะไม่วาดบล็อกนั้นเลย
//! ข้อมูล reasoning ยังถูกเก็บใน message state ตามปกติ (ไม่มีอะไรหาย — เปิดกลับมาเห็นได้)

const SHOW_THINKING_KEY = "thclaws.chat.showThinking";

export function isShowThinking(): boolean {
  try {
    return localStorage.getItem(SHOW_THINKING_KEY) === "1";
  } catch {
    return false; // private mode — ใช้ค่าเริ่มต้น (ซ่อน)
  }
}

export function setShowThinking(value: boolean) {
  try {
    if (value) localStorage.setItem(SHOW_THINKING_KEY, "1");
    else localStorage.removeItem(SHOW_THINKING_KEY);
  } catch {
    // เขียนไม่ได้ (private mode) — ค่าจะไม่คงอยู่ข้ามการเปิดแอป
  }
}
