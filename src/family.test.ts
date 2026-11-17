import { describe, expect, it } from "vitest";
import { familyColor, severityColor, severityOf } from "./family";
import { TRACE_COLORS } from "./plot";
import { AMBER, CORAL, CYAN, GREEN, LIME, PINK, RED, SKY, VIOLET, YELLOW } from "./theme";

describe("familyColor", () => {
  it("paints a math channel lime even when the name looks like another family", () => {
    expect(familyColor("BrakePressure", "Math")).toBe(LIME);
    expect(familyColor("BrakePressure", "MaTh")).toBe(LIME);
  });

  it("matches whole terms, including a camelCase pair run together", () => {
    expect(familyColor("PackVoltage")).toBe(VIOLET);
    expect(familyColor("CellTemp")).toBe(VIOLET);
    expect(familyColor("ReturnTemp")).toBe(TRACE_COLORS[0]);
    expect(familyColor("TurnLeft")).toBe(GREEN);
  });

  it("applies earlier families before later ones", () => {
    expect(familyColor("ChecksumCounter")).toBe(CORAL);
    expect(familyColor("Counter", "ECM")).toBe(PINK);
    expect(familyColor("Speed", "CounterMsg")).toBe(TRACE_COLORS[0]);
    expect(familyColor("BrakeRPM")).toBe(AMBER);
    expect(familyColor("OilPressure", "ABS")).toBe(AMBER);
    expect(familyColor("Foo", "ECM")).toBe(CYAN);
    expect(familyColor("VehicleSpeed")).toBe(YELLOW);
    expect(familyColor("DoorOpen")).toBe(GREEN);
    expect(familyColor("BMS_Pack")).toBe(VIOLET);
  });

  it("wraps the trace palette, including a negative index", () => {
    expect(familyColor("Nope", "", 8)).toBe(TRACE_COLORS[8 % TRACE_COLORS.length]);
    expect(familyColor("Nope", "", -1)).toBe(TRACE_COLORS[Math.abs(-1) % TRACE_COLORS.length]);
  });
});

describe("severityOf", () => {
  it("ranks a fault above a warning and leaves the rest as info", () => {
    expect(severityOf("ChecksumABS")).toBe("fault");
    expect(severityOf("Bus Off")).toBe("fault");
    expect(severityOf("bus-off")).toBe("fault");
    expect(severityOf("Error_Frame")).toBe("fault");
    expect(severityOf("DTC_P0301")).toBe("fault");
    expect(severityOf("Timeout")).toBe("fault");
    expect(severityOf("Missing")).toBe("fault");
    expect(severityOf("ABS_Trigger")).toBe("warn");
    expect(severityOf("CounterSkip")).toBe("warn");
    expect(severityOf("ESC")).toBe("warn");
    expect(severityOf("DoorOpen")).toBe("info");
    expect(severityOf("MIL")).toBe("info");
    expect(severityOf("")).toBe("info");
  });
});

describe("severityColor", () => {
  it("maps each severity to its hue", () => {
    expect(severityColor("fault")).toBe(RED);
    expect(severityColor("warn")).toBe(AMBER);
    expect(severityColor("info")).toBe(SKY);
  });
});
