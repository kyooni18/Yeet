import React, { useState, createContext, useContext } from "react";
import {
  Modal,
  Pressable,
  ScrollView,
  Text,
  TextInput,
  View,
} from "react-native";
import { layout, useTheme } from "../theme/theme";
export const ControlsDisabled = createContext(false);
export function Button({
  label,
  onPress,
  disabled = false,
  selected = false,
}: {
  label: string;
  onPress: () => void;
  disabled?: boolean;
  selected?: boolean;
}) {
  const c = useTheme();
  disabled = useContext(ControlsDisabled) || disabled;
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={label}
      accessibilityState={{ disabled, selected }}
      disabled={disabled}
      onPress={onPress}
      style={({ pressed }) => ({
        paddingHorizontal: 12,
        paddingVertical: 10,
        minHeight: 44,
        borderRadius: 8,
        backgroundColor: selected ? c.accent : c.surfaceRaised,
        opacity: disabled ? 0.45 : pressed ? 0.7 : 1,
      })}
    >
      <Text style={{ color: selected ? c.background : c.text, fontSize: 13 }}>
        {label}
      </Text>
    </Pressable>
  );
}
export function Label({
  children,
  muted = false,
}: {
  children: React.ReactNode;
  muted?: boolean;
}) {
  const c = useTheme();
  return (
    <Text
      selectable
      style={[layout.body, { color: muted ? c.textDim : c.text }]}
    >
      {children}
    </Text>
  );
}
export function Field({
  label,
  value,
  onChange,
  secure = false,
  multiline = false,
}: {
  label: string;
  value: string;
  onChange: (s: string) => void;
  secure?: boolean;
  multiline?: boolean;
}) {
  const c = useTheme();
  return (
    <View style={{ gap: 6 }}>
      <Label muted>{label}</Label>
      <TextInput
        editable={!useContext(ControlsDisabled)}
        accessibilityLabel={label}
        value={value}
        onChangeText={onChange}
        secureTextEntry={secure}
        multiline={multiline}
        autoCapitalize="none"
        autoCorrect={false}
        style={{
          borderWidth: 1,
          borderColor: c.border,
          borderRadius: 8,
          padding: 12,
          color: c.text,
          backgroundColor: c.background,
          minHeight: 44,
        }}
      />
    </View>
  );
}
export function Choice({
  label,
  value,
  options,
  onChange,
}: {
  label: string;
  value: string;
  options: string[];
  onChange: (s: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const c = useTheme();
  return (
    <>
      <Button
        label={`${label}: ${value || "Choose"}`}
        onPress={() => setOpen(true)}
      />
      <Modal
        visible={open}
        transparent
        animationType="fade"
        onRequestClose={() => setOpen(false)}
      >
        <View
          style={{
            flex: 1,
            backgroundColor: "#0009",
            justifyContent: "center",
            padding: 24,
          }}
        >
          <View
            accessibilityViewIsModal
            style={{
              backgroundColor: c.surface,
              borderRadius: 12,
              padding: 16,
              maxHeight: "80%",
              gap: 12,
            }}
          >
            <Label>{label}</Label>
            <ScrollView>
              {options.map((option) => (
                <View key={option} style={{ marginBottom: 8 }}>
                  <Button
                    label={option}
                    selected={option === value}
                    onPress={() => {
                      onChange(option);
                      setOpen(false);
                    }}
                  />
                </View>
              ))}
            </ScrollView>
            <Button label="Close" onPress={() => setOpen(false)} />
          </View>
        </View>
      </Modal>
    </>
  );
}
