import React, { createContext, useContext } from "react";
import { StyleSheet, useColorScheme } from "react-native";
import { useRemote } from "../state/client";
export const fallback = {
  background: "#101217",
  surface: "#191c23",
  surfaceRaised: "#242832",
  text: "#eef0f5",
  textDim: "#9ca5b5",
  border: "#343a47",
  accent: "#8fa8ff",
  error: "#ff8791",
  userSurface: "#253249",
};
const ThemeContext = createContext(fallback);
export function ThemeProvider({ children }: { children: React.ReactNode }) {
  const { state } = useRemote();
  const system = useColorScheme();
  const settings = state.runtime_settings;
  const light =
    settings?.appearance === "light" ||
    (settings?.appearance === "auto" && system === "light");
  const palette = light
    ? settings?.themeLightPalette
    : settings?.themeDarkPalette;
  return (
    <ThemeContext.Provider value={palette ?? fallback}>
      {children}
    </ThemeContext.Provider>
  );
}
export const useTheme = () => useContext(ThemeContext);
export const layout = StyleSheet.create({
  row: { flexDirection: "row", alignItems: "center", gap: 8, flexWrap: "wrap" },
  grow: { flex: 1, minWidth: 0 },
  panel: { padding: 12, gap: 12 },
  title: { fontSize: 18, fontWeight: "600" },
  label: { fontSize: 12, fontWeight: "600" },
  body: { fontSize: 14, lineHeight: 21 },
});
