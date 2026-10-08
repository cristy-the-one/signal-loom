import { RAIL_TABS, tabs, type RailTab } from "../dom";
import { state } from "../state";

export function showTab(tab: RailTab): void {
  state.tab = tab;
  for (const name of RAIL_TABS) {
    tabs[name].panel.hidden = name !== tab;
    tabs[name].button.classList.toggle("is-on", name === tab);
  }
}

export function bindTabs(): void {
  for (const name of RAIL_TABS) {
    tabs[name].button.addEventListener("click", () => showTab(name));
  }
}
