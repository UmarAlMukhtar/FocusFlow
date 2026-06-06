import { type CSSProperties, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

type RecordingPhase = "idle" | "recording" | "finalizing" | "completed";

type RecordingStatus = {
  phase: RecordingPhase;
  outputPath: string;
  pid: number | null;
  elapsedMs: number;
  fileSizeBytes: number | null;
};

type RecorderError = {
  code?: string;
  message?: string;
  recoverable?: boolean;
};

type ExportStatus = {
  inputPath: string;
  timelinePath: string;
  outputPath: string;
  segmentCount: number;
};

type ExportSettings = {
  zoomScale: number;
  zoomInMs: number;
  zoomOutMs: number;
  panTransitionMs: number;
};

const EXPORT_SETTINGS_STORAGE_KEY = "focusflow.exportSettings";
const DEFAULT_EXPORT_SETTINGS: ExportSettings = {
  zoomScale: 2.3,
  zoomInMs: 180,
  zoomOutMs: 220,
  panTransitionMs: 180,
};

function App() {
  const [status, setStatus] = useState<RecordingStatus | null>(null);
  const [isBusy, setIsBusy] = useState(false);
  const [countdown, setCountdown] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [exportMessage, setExportMessage] = useState<string | null>(null);
  const [exportOutputPath, setExportOutputPath] = useState<string | null>(null);
  const [exportSettings, setExportSettings] = useState<ExportSettings>(() =>
    loadExportSettings(),
  );

  const statusLabel = useMemo(() => {
    if (countdown !== null) return "Starting...";
    if (isBusy) return "Working...";
    if (!status) return "Unknown";
    return formatPhase(status.phase);
  }, [countdown, isBusy, status]);

  const isRecording = status?.phase === "recording";
  const currentSessionPath = useMemo(
    () => sessionPathFromOutput(status?.outputPath ?? ""),
    [status?.outputPath],
  );

  useEffect(() => {
    void refreshStatus();
  }, []);

  useEffect(() => {
    saveExportSettings(exportSettings);
  }, [exportSettings]);

  async function refreshStatus(clearError = true) {
    try {
      const nextStatus = await invoke<RecordingStatus>("recording_status");
      setStatus(nextStatus);
      if (clearError) {
        setError(null);
      }
    } catch (caught) {
      setError(formatError(caught));
    }
  }

  async function startRecording() {
    setIsBusy(true);
    setCountdown(null);
    setError(null);
    setExportMessage(null);

    try {
      for (const nextCountdown of [3, 2, 1]) {
        setCountdown(nextCountdown);
        await wait(1000);
      }

      setCountdown(null);
      const nextStatus = await invoke<RecordingStatus>("start_recording");
      setStatus(nextStatus);
    } catch (caught) {
      setError(formatError(caught));
      await refreshStatus(false);
    } finally {
      setCountdown(null);
      setIsBusy(false);
    }
  }

  async function stopRecording() {
    setIsBusy(true);
    setError(null);

    try {
      const nextStatus = await invoke<RecordingStatus>("stop_recording");
      setStatus(nextStatus);
    } catch (caught) {
      setError(formatError(caught));
      await refreshStatus(false);
    } finally {
      setIsBusy(false);
    }
  }

  async function exportEditedVideo() {
    setIsBusy(true);
    setError(null);
    setExportMessage(null);
    setExportOutputPath(null);

    try {
      const exportStatus = await invoke<ExportStatus>("export_edited_mp4", {
        settings: exportSettings,
      });
      setExportMessage("Export completed.");
      setExportOutputPath(exportStatus.outputPath);
    } catch (caught) {
      setError(formatError(caught));
    } finally {
      setIsBusy(false);
    }
  }

  function updateExportSetting(key: keyof ExportSettings, value: number) {
    setExportSettings((current) => ({
      ...current,
      [key]: value,
    }));
  }

  return (
    <main style={styles.page}>
      <section style={styles.shell}>
        <header style={styles.header}>
          <div>
            <p style={styles.eyebrow}>FocusFlow</p>
            <h1 style={styles.title}>Screen Recorder</h1>
          </div>
          <span
            style={{
              ...styles.statusPill,
              ...(isRecording ? styles.statusPillLive : {}),
            }}
          >
            {statusLabel}
          </span>
        </header>

        {countdown !== null ? (
          <div aria-live="assertive" style={styles.countdown}>
            {countdown}
          </div>
        ) : null}

        <section style={styles.statusPanel}>
          <div>
            <p style={styles.label}>Recording status</p>
            <p style={styles.statusValue}>{statusLabel}</p>
          </div>
          <div>
            <p style={styles.label}>Current session path</p>
            <p style={styles.pathValue}>
              {currentSessionPath || "No active session"}
            </p>
          </div>
        </section>

        <section style={styles.settingsPanel}>
          <div style={styles.settingsHeader}>
            <div>
              <p style={styles.label}>Export settings</p>
              <p style={styles.settingsTitle}>Auto zoom tuning</p>
            </div>
            <button
              type="button"
              onClick={() => setExportSettings(DEFAULT_EXPORT_SETTINGS)}
              disabled={isBusy}
              style={{
                ...styles.resetButton,
                ...(isBusy ? styles.disabledButton : {}),
              }}
            >
              Reset
            </button>
          </div>

          <div style={styles.settingsGrid}>
            <label style={styles.settingField}>
              <span style={styles.settingLabel}>Zoom Scale</span>
              <input
                type="number"
                min="1.1"
                max="6"
                step="0.1"
                value={exportSettings.zoomScale}
                onChange={(event) =>
                  updateExportSetting(
                    "zoomScale",
                    parseNumberInput(
                      event.currentTarget.value,
                      DEFAULT_EXPORT_SETTINGS.zoomScale,
                    ),
                  )
                }
                style={styles.numberInput}
              />
            </label>

            <label style={styles.settingField}>
              <span style={styles.settingLabel}>Zoom In Duration</span>
              <input
                type="number"
                min="1"
                step="10"
                value={exportSettings.zoomInMs}
                onChange={(event) =>
                  updateExportSetting(
                    "zoomInMs",
                    parseNumberInput(
                      event.currentTarget.value,
                      DEFAULT_EXPORT_SETTINGS.zoomInMs,
                    ),
                  )
                }
                style={styles.numberInput}
              />
            </label>

            <label style={styles.settingField}>
              <span style={styles.settingLabel}>Zoom Out Duration</span>
              <input
                type="number"
                min="1"
                step="10"
                value={exportSettings.zoomOutMs}
                onChange={(event) =>
                  updateExportSetting(
                    "zoomOutMs",
                    parseNumberInput(
                      event.currentTarget.value,
                      DEFAULT_EXPORT_SETTINGS.zoomOutMs,
                    ),
                  )
                }
                style={styles.numberInput}
              />
            </label>

            <label style={styles.settingField}>
              <span style={styles.settingLabel}>Pan Transition Duration</span>
              <input
                type="number"
                min="1"
                step="10"
                value={exportSettings.panTransitionMs}
                onChange={(event) =>
                  updateExportSetting(
                    "panTransitionMs",
                    parseNumberInput(
                      event.currentTarget.value,
                      DEFAULT_EXPORT_SETTINGS.panTransitionMs,
                    ),
                  )
                }
                style={styles.numberInput}
              />
            </label>
          </div>
        </section>

        <div style={styles.controls}>
          <button
            type="button"
            onClick={startRecording}
            disabled={isBusy || isRecording}
            style={{
              ...styles.startButton,
              ...(isBusy || isRecording ? styles.disabledButton : {}),
            }}
          >
            Start Recording
          </button>

          <button
            type="button"
            onClick={stopRecording}
            disabled={isBusy || !isRecording}
            style={{
              ...styles.secondaryButton,
              ...(isBusy || !isRecording ? styles.disabledButton : {}),
            }}
          >
            Stop Recording
          </button>

          <button
            type="button"
            onClick={exportEditedVideo}
            disabled={isBusy || isRecording}
            style={{
              ...styles.secondaryButton,
              ...(isBusy || isRecording ? styles.disabledButton : {}),
            }}
          >
            Export
          </button>
        </div>

        {status?.outputPath ? (
          <p style={styles.meta}>Screen file: {status.outputPath}</p>
        ) : null}

        {exportMessage ? <p style={styles.success}>{exportMessage}</p> : null}

        {exportOutputPath ? (
          <p style={styles.meta}>Edited output: {exportOutputPath}</p>
        ) : null}

        {error ? <p style={styles.error}>{error}</p> : null}
      </section>
    </main>
  );
}

function formatPhase(phase: RecordingPhase) {
  return phase
    .split("_")
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
    .join(" ");
}

function formatError(caught: unknown) {
  if (typeof caught === "string") return caught;

  if (caught && typeof caught === "object") {
    const recorderError = caught as RecorderError;
    if (recorderError.message) return recorderError.message;
    if (recorderError.code) return recorderError.code;
  }

  return "Recorder command failed";
}

function sessionPathFromOutput(outputPath: string) {
  if (!outputPath) return "";

  const lastSeparator = Math.max(
    outputPath.lastIndexOf("\\"),
    outputPath.lastIndexOf("/"),
  );

  return lastSeparator >= 0 ? outputPath.slice(0, lastSeparator) : outputPath;
}

function loadExportSettings(): ExportSettings {
  try {
    const rawSettings = window.localStorage.getItem(EXPORT_SETTINGS_STORAGE_KEY);
    if (!rawSettings) return DEFAULT_EXPORT_SETTINGS;

    return normalizeExportSettings(JSON.parse(rawSettings));
  } catch {
    return DEFAULT_EXPORT_SETTINGS;
  }
}

function saveExportSettings(settings: ExportSettings) {
  try {
    window.localStorage.setItem(
      EXPORT_SETTINGS_STORAGE_KEY,
      JSON.stringify(settings),
    );
  } catch {
    // Ignore storage failures; export still uses the in-memory settings.
  }
}

function normalizeExportSettings(value: unknown): ExportSettings {
  if (!value || typeof value !== "object") {
    return DEFAULT_EXPORT_SETTINGS;
  }

  const settings = value as Partial<Record<keyof ExportSettings, unknown>>;

  return {
    zoomScale: positiveNumberOrDefault(
      settings.zoomScale,
      DEFAULT_EXPORT_SETTINGS.zoomScale,
    ),
    zoomInMs: positiveNumberOrDefault(
      settings.zoomInMs,
      DEFAULT_EXPORT_SETTINGS.zoomInMs,
    ),
    zoomOutMs: positiveNumberOrDefault(
      settings.zoomOutMs,
      DEFAULT_EXPORT_SETTINGS.zoomOutMs,
    ),
    panTransitionMs: positiveNumberOrDefault(
      settings.panTransitionMs,
      DEFAULT_EXPORT_SETTINGS.panTransitionMs,
    ),
  };
}

function positiveNumberOrDefault(value: unknown, fallback: number) {
  return typeof value === "number" && Number.isFinite(value) && value > 0
    ? value
    : fallback;
}

function parseNumberInput(value: string, fallback: number) {
  const parsed = Number(value);

  return Number.isFinite(parsed) && parsed > 0 ? parsed : fallback;
}

function wait(milliseconds: number) {
  return new Promise<void>((resolve) => {
    window.setTimeout(resolve, milliseconds);
  });
}

const styles = {
  page: {
    minHeight: "100vh",
    display: "grid",
    placeItems: "center",
    padding: "32px",
    background: "#0b0d10",
    color: "#f4f7fb",
    fontFamily:
      "Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, Segoe UI, sans-serif",
  },
  shell: {
    width: "min(760px, 100%)",
    padding: "28px",
    border: "1px solid #262b33",
    borderRadius: "8px",
    background: "#12151a",
    boxShadow: "0 24px 72px rgba(0, 0, 0, 0.38)",
  },
  header: {
    display: "flex",
    alignItems: "flex-start",
    justifyContent: "space-between",
    gap: "20px",
    marginBottom: "24px",
  },
  eyebrow: {
    margin: "0 0 6px",
    color: "#34d399",
    fontSize: "13px",
    fontWeight: 800,
    letterSpacing: "0",
    textTransform: "uppercase",
  },
  title: {
    margin: 0,
    fontSize: "34px",
    lineHeight: 1.05,
    fontWeight: 800,
  },
  statusPill: {
    minHeight: "34px",
    display: "inline-flex",
    alignItems: "center",
    justifyContent: "center",
    padding: "0 12px",
    border: "1px solid #353b45",
    borderRadius: "999px",
    background: "#1b2027",
    color: "#d7dde7",
    fontSize: "13px",
    fontWeight: 700,
    whiteSpace: "nowrap",
  },
  statusPillLive: {
    borderColor: "#2f8f61",
    background: "#0d2a1d",
    color: "#8ff0b8",
  },
  countdown: {
    minHeight: "132px",
    display: "grid",
    placeItems: "center",
    margin: "0 0 18px",
    border: "1px solid #2f8f61",
    borderRadius: "8px",
    background: "#0d2a1d",
    color: "#bbf7d0",
    fontSize: "72px",
    fontWeight: 800,
  },
  statusPanel: {
    display: "grid",
    gridTemplateColumns: "180px minmax(0, 1fr)",
    gap: "18px",
    padding: "18px",
    border: "1px solid #262b33",
    borderRadius: "8px",
    background: "#171b21",
    marginBottom: "20px",
  },
  settingsPanel: {
    padding: "18px",
    border: "1px solid #262b33",
    borderRadius: "8px",
    background: "#15191f",
    marginBottom: "20px",
  },
  settingsHeader: {
    display: "flex",
    alignItems: "center",
    justifyContent: "space-between",
    gap: "16px",
    marginBottom: "16px",
  },
  settingsTitle: {
    margin: 0,
    color: "#f4f7fb",
    fontSize: "18px",
    fontWeight: 800,
  },
  settingsGrid: {
    display: "grid",
    gridTemplateColumns: "repeat(4, minmax(0, 1fr))",
    gap: "12px",
  },
  settingField: {
    display: "grid",
    gap: "8px",
    minWidth: 0,
  },
  settingLabel: {
    color: "#aab3c1",
    fontSize: "12px",
    fontWeight: 700,
    lineHeight: 1.3,
  },
  numberInput: {
    width: "100%",
    minHeight: "40px",
    boxSizing: "border-box",
    border: "1px solid #343b46",
    borderRadius: "6px",
    background: "#0f1318",
    color: "#f4f7fb",
    padding: "0 10px",
    font: "inherit",
    fontSize: "14px",
    fontWeight: 700,
  },
  resetButton: {
    minHeight: "34px",
    padding: "0 12px",
    border: "1px solid #343b46",
    borderRadius: "6px",
    background: "#20252d",
    color: "#f4f7fb",
    font: "inherit",
    fontSize: "13px",
    fontWeight: 800,
    cursor: "pointer",
  },
  label: {
    margin: "0 0 8px",
    color: "#8b95a5",
    fontSize: "12px",
    fontWeight: 700,
    letterSpacing: "0",
    textTransform: "uppercase",
  },
  statusValue: {
    margin: 0,
    color: "#f4f7fb",
    fontSize: "22px",
    fontWeight: 800,
  },
  pathValue: {
    margin: 0,
    color: "#d7dde7",
    fontFamily:
      "ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, Liberation Mono, monospace",
    fontSize: "13px",
    lineHeight: 1.5,
    overflowWrap: "anywhere",
  },
  controls: {
    display: "grid",
    gridTemplateColumns: "minmax(220px, 1.35fr) repeat(2, minmax(120px, 0.65fr))",
    gap: "12px",
  },
  startButton: {
    minHeight: "68px",
    padding: "0 22px",
    border: "1px solid #4ade80",
    borderRadius: "6px",
    background: "#22c55e",
    color: "#06120b",
    font: "inherit",
    fontSize: "18px",
    fontWeight: 800,
    cursor: "pointer",
  },
  secondaryButton: {
    minHeight: "68px",
    padding: "0 16px",
    border: "1px solid #343b46",
    borderRadius: "6px",
    background: "#20252d",
    color: "#f4f7fb",
    font: "inherit",
    fontSize: "15px",
    fontWeight: 800,
    cursor: "pointer",
  },
  disabledButton: {
    opacity: 0.45,
    cursor: "not-allowed",
  },
  meta: {
    margin: "16px 0 0",
    fontSize: "13px",
    lineHeight: 1.5,
    color: "#aab3c1",
    overflowWrap: "anywhere",
  },
  error: {
    margin: "16px 0 0",
    fontSize: "14px",
    lineHeight: 1.5,
    color: "#fca5a5",
  },
  success: {
    margin: "16px 0 0",
    fontSize: "14px",
    lineHeight: 1.5,
    color: "#86efac",
  },
} satisfies Record<string, CSSProperties>;

export default App;
