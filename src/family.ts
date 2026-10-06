import { TRACE_COLORS } from "./plot";

export type Severity = "fault" | "warn" | "info";

/** Colour a signal by the ECU family it belongs to. */
export function familyColor(name: string, message = "", index = 0): string {
  const blob = `${name} ${message}`.toLowerCase();
  if (message.toLowerCase() === "math") return "#b6e36a";
  if (/checksum|dtc|mil|timeout|missing/.test(blob)) return "#ff7a59";
  if (/counter/.test(name.toLowerCase())) return "#ff8cc6";
  if (/bms|soc|packvoltage|packcurrent|celltemp|fuellevel|fuel/.test(blob)) return "#c9a0ff";
  if (/door|turn|lamp|telltale|bcm/.test(blob)) return "#7ddea5";
  if (/abs|esc|brake|wheel|steer|latg|yaw/.test(blob)) return "#e6a23c";
  if (/vehiclespeed|displayedr|cluster|ic_/.test(blob)) return "#f2e394";
  if (/rpm|torque|throttle|coolant|oil|gear|pedal|clutch|ecm|tcu/.test(blob)) return "#3ec6ff";
  return TRACE_COLORS[Math.abs(index) % TRACE_COLORS.length];
}

export function severityOf(label: string): Severity {
  const text = label.toLowerCase();
  if (/checksum|bus-off|bus off|error frame|dtc|timeout|missing/.test(text)) return "fault";
  if (/counter|abs|trigger|esc/.test(text)) return "warn";
  return "info";
}

export function severityColor(kind: Severity): string {
  if (kind === "fault") return "#ff5a45";
  if (kind === "warn") return "#e6a23c";
  return "#7eb6ff";
}
