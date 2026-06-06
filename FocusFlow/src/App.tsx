import { type CSSProperties, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import "./App.css";

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

type ExportProgress = {
  percentage: number;
};

type ExportSettings = {
  zoomScale: number;
  zoomInMs: number;
  zoomOutMs: number;
  panTransitionMs: number;
};

const EXPORT_PROGRESS_EVENT = "export-progress";
const RECORDING_STATUS_CHANGED_EVENT = "recording-status-changed";
const EXPORT_SETTINGS_STORAGE_KEY = "focusflow.exportSettings";
const DEFAULT_EXPORT_SETTINGS: ExportSettings = {
  zoomScale: 2.3,
  zoomInMs: 180,
  zoomOutMs: 220,
  panTransitionMs: 180,
};
const FRIENDLY_ERROR_MESSAGES: Record<string, string> = {
  app_data_dir_unavailable:
    "FocusFlow could not find its app data folder. Restart the app and try again.",
  click_tracker_failed:
    "FocusFlow could not finish collecting click data for this recording.",
  click_tracker_state_poisoned:
    "FocusFlow could not read the captured click data. Start a new recording and try again.",
  create_output_dir_failed:
    "FocusFlow could not create the recordings folder. Check disk access and try again.",
  create_session_dir_failed:
    "FocusFlow could not create a new session folder. Check disk access and try again.",
  drag_tracker_state_poisoned:
    "FocusFlow could not read the captured drag data. Start a new recording and try again.",
  export_join_failed:
    "The export task stopped unexpectedly. Try exporting the session again.",
  ffmpeg_export_failed:
    "FocusFlow could not render the edited video. Try exporting again.",
  ffmpeg_probe_failed:
    "FocusFlow could not read the recorded video metadata.",
  ffmpeg_probe_spawn_failed:
    "FocusFlow could not start bundled FFmpeg to inspect the recording.",
  ffmpeg_recording_failed:
    "Screen recording stopped with an FFmpeg error. Try recording again.",
  ffmpeg_sidecar_unavailable:
    "Bundled FFmpeg is missing or unavailable. Reinstall FocusFlow and try again.",
  ffmpeg_spawn_failed:
    "FocusFlow could not start bundled FFmpeg. Restart the app and try again.",
  ffmpeg_stop_timeout:
    "FocusFlow could not stop FFmpeg cleanly. Start a new recording and try again.",
  input_missing:
    "Required session files are missing. Record a new session and try again.",
  non_utf8_path:
    "One of the session paths contains unsupported characters.",
  parse_clicks_failed:
    "FocusFlow could not read the click data for this session.",
  parse_drags_failed:
    "FocusFlow could not read the drag data for this session.",
  parse_timeline_failed:
    "FocusFlow could not read the zoom timeline for this session.",
  primary_monitor_unavailable:
    "FocusFlow could not find the primary monitor.",
  primary_monitor_query_failed:
    "FocusFlow could not access monitor information.",
  read_clicks_failed:
    "FocusFlow could not open the click data for this session.",
  read_drags_failed:
    "FocusFlow could not open the drag data for this session.",
  read_recordings_dir_failed:
    "FocusFlow could not read the recordings folder.",
  read_timeline_failed:
    "FocusFlow could not open the zoom timeline for this session.",
  recorder_already_running: "A recording is already in progress.",
  recorder_finalizing:
    "The previous recording is still finishing. Try again in a moment.",
  recorder_not_running: "No recording is currently running.",
  recorder_state_poisoned:
    "The recorder state could not be read. Restart FocusFlow and try again.",
  recording_output_empty:
    "Recording finished, but the saved video was empty. Try recording again.",
  recording_output_missing:
    "Recording finished, but the saved video could not be found.",
  recording_session_missing:
    "No completed recording session is ready yet. Record a session first.",
  relative_output_dir:
    "FocusFlow resolved an invalid recordings folder. Restart the app and try again.",
  remove_file_failed:
    "FocusFlow could not clean up a temporary export file. Close the video if it is open and try again.",
  replace_output_failed:
    "FocusFlow could not save edited.mp4. Close the video if it is open and try again.",
  unsupported_platform:
    "This FocusFlow build currently supports recording and export on Windows.",
  video_dimensions_missing:
    "FocusFlow could not read the recorded video size.",
  video_duration_missing:
    "FocusFlow could not read the recorded video duration.",
  write_clicks_failed:
    "FocusFlow could not save click data for this recording.",
  write_drags_failed:
    "FocusFlow could not save drag data for this recording.",
  write_filter_script_failed:
    "FocusFlow could not prepare the export filter file. Check disk access and try again.",
  write_timeline_failed:
    "FocusFlow could not save the zoom timeline for this recording.",
};

function App() {
  const [status, setStatus] = useState<RecordingStatus | null>(null);
  const [isBusy, setIsBusy] = useState(false);
  const [isExporting, setIsExporting] = useState(false);
  const [countdown, setCountdown] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [exportMessage, setExportMessage] = useState<string | null>(null);
  const [exportOutputPath, setExportOutputPath] = useState<string | null>(null);
  const [exportProgressPercentage, setExportProgressPercentage] = useState<
    number | null
  >(null);
  const [exportSettings, setExportSettings] = useState<ExportSettings>(() =>
    loadExportSettings(),
  );

  const statusLabel = useMemo(() => {
    if (countdown !== null) return "Starting...";
    if (isExporting) return "Exporting...";
    if (isBusy) return "Working...";
    if (!status) return "Unknown";
    return formatPhase(status.phase);
  }, [countdown, isBusy, isExporting, status]);

  const isRecording = status?.phase === "recording";
  const elapsedLabel = formatElapsedTime(status?.elapsedMs ?? 0);
  const currentSessionPath = useMemo(
    () => sessionPathFromOutput(status?.outputPath ?? ""),
    [status?.outputPath],
  );
  const editedVideoFolderPath = useMemo(
    () => sessionPathFromOutput(exportOutputPath ?? ""),
    [exportOutputPath],
  );

  useEffect(() => {
    void refreshStatus();
  }, []);

  useEffect(() => {
    let isDisposed = false;
    let unlisten: (() => void) | null = null;

    void listen(RECORDING_STATUS_CHANGED_EVENT, () => {
      void refreshStatus(false);
    })
      .then((nextUnlisten) => {
        if (isDisposed) {
          nextUnlisten();
          return;
        }

        unlisten = nextUnlisten;
      })
      .catch((caught) => {
        if (!isDisposed) {
          setError(`Recording hotkeys unavailable: ${formatError(caught)}`);
        }
      });

    return () => {
      isDisposed = true;
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    let isDisposed = false;
    let unlisten: (() => void) | null = null;

    void listen<ExportProgress>(EXPORT_PROGRESS_EVENT, (event) => {
      const percentage = normalizeProgressPercentage(event.payload.percentage);

      if (percentage !== null) {
        setExportProgressPercentage(percentage);
      }
    })
      .then((nextUnlisten) => {
        if (isDisposed) {
          nextUnlisten();
          return;
        }

        unlisten = nextUnlisten;
      })
      .catch((caught) => {
        if (!isDisposed) {
          setError(`Export progress unavailable: ${formatError(caught)}`);
        }
      });

    return () => {
      isDisposed = true;
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    if (!isRecording) return;

    const intervalId = window.setInterval(() => {
      void refreshStatus(false);
    }, 1000);

    return () => {
      window.clearInterval(intervalId);
    };
  }, [isRecording]);

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
    let didMinimizeWindow = false;

    try {
      for (const nextCountdown of [3, 2, 1]) {
        setCountdown(nextCountdown);
        await wait(1000);
      }

      setCountdown(null);
      await minimizeFocusFlowWindow();
      didMinimizeWindow = true;

      const nextStatus = await invoke<RecordingStatus>("start_recording");
      setStatus(nextStatus);
    } catch (caught) {
      if (didMinimizeWindow) {
        await restoreFocusFlowWindow();
      }
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
      await restoreFocusFlowWindow();
    } catch (caught) {
      setError(formatError(caught));
      await refreshStatus(false);
      await restoreFocusFlowWindow();
    } finally {
      setIsBusy(false);
    }
  }

  async function exportEditedVideo() {
    setIsBusy(true);
    setError(null);
    setExportMessage(null);
    setExportOutputPath(null);
    setExportProgressPercentage(0);
    setIsExporting(true);

    try {
      const exportStatus = await invoke<ExportStatus>("export_edited_mp4", {
        settings: exportSettings,
      });
      setExportMessage("Export completed.");
      setExportOutputPath(exportStatus.outputPath);
      setExportProgressPercentage(100);
    } catch (caught) {
      setError(formatError(caught));
      setExportProgressPercentage(null);
    } finally {
      setIsExporting(false);
      setIsBusy(false);
    }
  }

  function updateExportSetting(key: keyof ExportSettings, value: number) {
    setExportSettings((current) => ({
      ...current,
      [key]: value,
    }));
  }

  async function openFolder(
    command: string,
    label: string,
    folderPath?: string | null,
  ) {
    try {
      setError(null);
      await invoke(
        command,
        folderPath === undefined ? undefined : { folderPath },
      );
    } catch (caught) {
      setError(`${label} could not be opened: ${formatError(caught)}`);
    }
  }

  return (
    <main style={styles.page}>
      <section style={styles.shell}>
        <header style={styles.header}>
          <div style={styles.brandBlock}>
            <div style={styles.brandMark}>
              <img
                src="/favicon.png"
                alt=""
                aria-hidden="true"
                style={styles.brandLogo}
              />
            </div>
            <div style={styles.brandText}>
              <p style={styles.eyebrow}>FocusFlow</p>
              <h1 style={styles.title}>Screen Recorder</h1>
              <p style={styles.subtitle}>
                Capture, zoom, and export focused demos.
              </p>
            </div>
          </div>
          <span
            style={{
              ...styles.statusPill,
              ...(isRecording ? styles.statusPillLive : {}),
            }}
          >
            {isRecording ? (
              <span aria-hidden="true" className="recording-live-dot" />
            ) : null}
            {statusLabel}
          </span>
        </header>

        <section style={styles.card}>
          <div style={styles.cardHeader}>
            <div>
              <p style={styles.label}>Session</p>
              <p style={styles.cardTitle}>Recording overview</p>
            </div>
          </div>

          <div style={styles.statusPanel}>
            <div style={styles.statTile}>
              <p style={styles.label}>Recording status</p>
              <p style={styles.statusValue}>{statusLabel}</p>
            </div>
            <div style={styles.statTile}>
              <p style={styles.label}>Elapsed time</p>
              <p
                style={{
                  ...styles.elapsedValue,
                  ...(isRecording ? styles.elapsedValueLive : {}),
                }}
              >
                {elapsedLabel}
              </p>
            </div>
          </div>
        </section>

        <section style={styles.card}>
          <div style={styles.cardHeader}>
            <div>
              <p style={styles.label}>Export settings</p>
              <p style={styles.cardTitle}>Auto zoom tuning</p>
            </div>
            <button
              type="button"
              onClick={() => setExportSettings(DEFAULT_EXPORT_SETTINGS)}
              disabled={isBusy}
              style={{
                ...styles.ghostButton,
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

        <section style={styles.card}>
          <div style={styles.cardHeader}>
            <div>
              <p style={styles.label}>Recorder</p>
              <p style={styles.cardTitle}>Session controls</p>
            </div>
          </div>

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

          {isExporting && exportProgressPercentage !== null ? (
            <div style={styles.exportProgress}>
              <div style={styles.progressHeader}>
                <span style={styles.settingLabel}>Export progress</span>
                <span style={styles.progressValue}>
                  {formatProgressPercentage(exportProgressPercentage)}
                </span>
              </div>
              <div
                aria-label="Export progress"
                aria-valuemax={100}
                aria-valuemin={0}
                aria-valuenow={Math.round(exportProgressPercentage)}
                role="progressbar"
                style={styles.progressTrack}
              >
                <div
                  style={{
                    ...styles.progressFill,
                    width: `${exportProgressPercentage}%`,
                  }}
                />
              </div>
            </div>
          ) : null}
        </section>

        <section style={styles.card}>
          <div style={styles.cardHeader}>
            <div>
              <p style={styles.label}>Output</p>
              <p style={styles.cardTitle}>Generated files</p>
            </div>
          </div>

          <div style={styles.folderActions}>
            <button
              type="button"
              onClick={() =>
                void openFolder("open_recordings_folder", "Recordings folder")
              }
              style={styles.folderButton}
            >
              Open Recordings Folder
            </button>

            <button
              type="button"
              onClick={() =>
                void openFolder(
                  "open_recording_session_folder",
                  "Session folder",
                  currentSessionPath || null,
                )
              }
              style={{
                ...styles.folderButton,
              }}
            >
              Open Session Folder
            </button>

            <button
              type="button"
              onClick={() =>
                void openFolder(
                  "open_edited_video_folder",
                  "Edited video folder",
                  editedVideoFolderPath,
                )
              }
              disabled={!editedVideoFolderPath}
              style={{
                ...styles.folderButton,
                ...(!editedVideoFolderPath ? styles.disabledButton : {}),
              }}
            >
              Open Edited Video Folder
            </button>
          </div>
        </section>

        {exportMessage ? <p style={styles.success}>{exportMessage}</p> : null}

        {error ? <p style={styles.error}>{error}</p> : null}
      </section>

      {countdown !== null ? (
        <div aria-live="assertive" style={styles.countdownOverlay}>
          <span style={styles.countdownValue}>{countdown}</span>
        </div>
      ) : null}
    </main>
  );
}

function formatPhase(phase: RecordingPhase) {
  return phase
    .split("_")
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
    .join(" ");
}

function formatElapsedTime(elapsedMs: number) {
  const totalSeconds = Math.max(0, Math.floor(elapsedMs / 1000));
  const hours = Math.floor(totalSeconds / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  const seconds = totalSeconds % 60;

  return [hours, minutes, seconds]
    .map((part) => part.toString().padStart(2, "0"))
    .join(":");
}

function formatError(caught: unknown) {
  if (typeof caught === "string") return friendlyStringError(caught);

  if (caught && typeof caught === "object") {
    const recorderError = caught as RecorderError;
    if (recorderError.code && FRIENDLY_ERROR_MESSAGES[recorderError.code]) {
      return FRIENDLY_ERROR_MESSAGES[recorderError.code];
    }

    if (recorderError.message) {
      return friendlyStringError(recorderError.message);
    }

    if (recorderError.code) {
      return "FocusFlow could not complete that action. Try again.";
    }
  }

  return "FocusFlow could not complete that action. Try again.";
}

function friendlyStringError(message: string) {
  if (message.includes("No recording session containing")) {
    return "No completed recording session is available yet.";
  }

  if (message.includes("Opening folders is only supported on Windows")) {
    return "Folder opening is only available on Windows.";
  }

  if (message.includes("Folder is outside the FocusFlow recordings directory")) {
    return "FocusFlow can only open folders inside its recordings directory.";
  }

  if (message.includes("Path is not a folder")) {
    return "That recording folder is no longer available.";
  }

  if (message.includes("Could not resolve folder")) {
    return "That recording folder is no longer available.";
  }

  if (message.includes("Could not read recording entry")) {
    return "FocusFlow could not read one of the saved recording sessions.";
  }

  if (message.includes("Could not open folder in File Explorer")) {
    return "File Explorer could not open that folder.";
  }

  if (message.includes("Could not resolve recordings directory")) {
    return "FocusFlow could not find the recordings folder. Restart the app and try again.";
  }

  if (message.includes("Could not read recordings directory")) {
    return "FocusFlow could not read the recordings folder.";
  }

  return "FocusFlow could not complete that action. Try again.";
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

function normalizeProgressPercentage(value: unknown) {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    return null;
  }

  return Math.max(0, Math.min(100, value));
}

function formatProgressPercentage(value: number) {
  return `${Math.floor(normalizeProgressPercentage(value) ?? 0)}%`;
}

function wait(milliseconds: number) {
  return new Promise<void>((resolve) => {
    window.setTimeout(resolve, milliseconds);
  });
}

async function minimizeFocusFlowWindow() {
  await getCurrentWindow().minimize();
}

async function restoreFocusFlowWindow() {
  const appWindow = getCurrentWindow();

  try {
    await appWindow.unminimize();
    await appWindow.setFocus();
  } catch (caught) {
    console.error("Could not restore FocusFlow window", caught);
  }
}

const styles = {
  page: {
    width: "100vw",
    height: "100vh",
    boxSizing: "border-box",
    display: "grid",
    placeItems: "center",
    overflow: "hidden",
    padding: "12px",
    background: "#09090b",
    color: "#fafafa",
    fontFamily:
      "Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, Segoe UI, sans-serif",
  },
  shell: {
    width: "min(760px, 100%)",
    maxHeight: "100%",
    boxSizing: "border-box",
    display: "grid",
    gap: "8px",
    overflow: "hidden",
    padding: "16px",
    border: "1px solid #27272a",
    borderRadius: "12px",
    background: "#0f1014",
    boxShadow: "0 24px 64px rgba(0, 0, 0, 0.42)",
  },
  header: {
    display: "flex",
    alignItems: "flex-start",
    justifyContent: "space-between",
    gap: "16px",
    minWidth: 0,
  },
  brandBlock: {
    display: "flex",
    alignItems: "center",
    gap: "12px",
    minWidth: 0,
  },
  brandText: {
    minWidth: 0,
  },
  brandMark: {
    width: "40px",
    height: "40px",
    display: "grid",
    placeItems: "center",
    flex: "0 0 auto",
    overflow: "hidden",
    border: "1px solid #2f3b35",
    borderRadius: "10px",
    background: "#09090b",
  },
  brandLogo: {
    width: "100%",
    height: "100%",
    display: "block",
    objectFit: "cover",
  },
  eyebrow: {
    margin: "0 0 2px",
    color: "#a1a1aa",
    fontSize: "11px",
    fontWeight: 700,
    letterSpacing: "0",
    textTransform: "uppercase",
  },
  title: {
    margin: 0,
    color: "#fafafa",
    fontSize: "24px",
    lineHeight: 1.05,
    fontWeight: 700,
  },
  subtitle: {
    margin: "4px 0 0",
    color: "#a1a1aa",
    fontSize: "13px",
    lineHeight: 1.35,
  },
  statusPill: {
    minHeight: "30px",
    display: "inline-flex",
    alignItems: "center",
    justifyContent: "center",
    gap: "8px",
    padding: "0 10px",
    border: "1px solid #27272a",
    borderRadius: "999px",
    background: "#18181b",
    color: "#e4e4e7",
    fontSize: "12px",
    fontWeight: 600,
    whiteSpace: "nowrap",
  },
  statusPillLive: {
    borderColor: "#7f1d1d",
    background: "#2a1113",
    color: "#fecaca",
  },
  countdownOverlay: {
    position: "fixed",
    inset: 0,
    zIndex: 10,
    display: "grid",
    placeItems: "center",
    background: "rgba(11, 13, 16, 0.88)",
    backdropFilter: "blur(4px)",
  },
  countdownValue: {
    width: "132px",
    height: "132px",
    display: "grid",
    placeItems: "center",
    border: "1px solid #2f8f61",
    borderRadius: "12px",
    background: "#0d2a1d",
    color: "#bbf7d0",
    fontSize: "72px",
    fontWeight: 800,
    lineHeight: 1,
  },
  card: {
    padding: "10px",
    border: "1px solid #27272a",
    borderRadius: "10px",
    background: "#18181b",
    minWidth: 0,
  },
  cardHeader: {
    display: "flex",
    alignItems: "center",
    justifyContent: "space-between",
    gap: "12px",
    marginBottom: "8px",
  },
  cardTitle: {
    margin: 0,
    color: "#fafafa",
    fontSize: "15px",
    fontWeight: 650,
    lineHeight: 1.2,
  },
  statusPanel: {
    display: "grid",
    gridTemplateColumns: "repeat(auto-fit, minmax(150px, 1fr))",
    gap: "10px",
  },
  statTile: {
    minHeight: "56px",
    padding: "9px",
    border: "1px solid #27272a",
    borderRadius: "8px",
    background: "#0f1014",
  },
  settingsGrid: {
    display: "grid",
    gridTemplateColumns: "repeat(auto-fit, minmax(120px, 1fr))",
    gap: "10px",
  },
  settingField: {
    display: "grid",
    gap: "6px",
    minWidth: 0,
  },
  settingLabel: {
    color: "#a1a1aa",
    fontSize: "11px",
    fontWeight: 600,
    lineHeight: 1.3,
  },
  numberInput: {
    width: "100%",
    minHeight: "34px",
    boxSizing: "border-box",
    border: "1px solid #27272a",
    borderRadius: "8px",
    background: "#09090b",
    color: "#fafafa",
    padding: "0 10px",
    font: "inherit",
    fontSize: "13px",
    fontWeight: 500,
  },
  ghostButton: {
    minHeight: "30px",
    padding: "0 12px",
    border: "1px solid #27272a",
    borderRadius: "8px",
    background: "#09090b",
    color: "#e4e4e7",
    font: "inherit",
    fontSize: "12px",
    fontWeight: 600,
    cursor: "pointer",
  },
  label: {
    margin: "0 0 6px",
    color: "#71717a",
    fontSize: "10px",
    fontWeight: 700,
    letterSpacing: "0",
    textTransform: "uppercase",
  },
  statusValue: {
    margin: 0,
    color: "#fafafa",
    fontSize: "18px",
    fontWeight: 650,
  },
  elapsedValue: {
    margin: 0,
    color: "#fafafa",
    fontFamily:
      "ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, Liberation Mono, monospace",
    fontSize: "20px",
    fontWeight: 700,
    lineHeight: 1.1,
    fontVariantNumeric: "tabular-nums",
  },
  elapsedValueLive: {
    color: "#fecaca",
  },
  controls: {
    display: "grid",
    gridTemplateColumns: "minmax(200px, 1.35fr) repeat(2, minmax(112px, 0.65fr))",
    gap: "8px",
  },
  folderActions: {
    display: "grid",
    gridTemplateColumns: "repeat(auto-fit, minmax(180px, 1fr))",
    gap: "8px",
  },
  exportProgress: {
    display: "grid",
    gap: "6px",
    marginTop: "10px",
  },
  progressHeader: {
    display: "flex",
    alignItems: "center",
    justifyContent: "space-between",
    gap: "12px",
  },
  progressValue: {
    color: "#e4e4e7",
    fontSize: "12px",
    fontWeight: 650,
    fontVariantNumeric: "tabular-nums",
  },
  progressTrack: {
    width: "100%",
    height: "8px",
    overflow: "hidden",
    border: "1px solid #27272a",
    borderRadius: "999px",
    background: "#09090b",
  },
  progressFill: {
    height: "100%",
    borderRadius: "999px",
    background: "#86efac",
    transition: "width 160ms ease",
  },
  startButton: {
    minHeight: "44px",
    padding: "0 18px",
    border: "1px solid #86efac",
    borderRadius: "8px",
    background: "#fafafa",
    color: "#09090b",
    font: "inherit",
    fontSize: "14px",
    fontWeight: 650,
    cursor: "pointer",
  },
  secondaryButton: {
    minHeight: "44px",
    padding: "0 14px",
    border: "1px solid #27272a",
    borderRadius: "8px",
    background: "#09090b",
    color: "#fafafa",
    font: "inherit",
    fontSize: "13px",
    fontWeight: 600,
    cursor: "pointer",
  },
  folderButton: {
    minHeight: "36px",
    padding: "0 14px",
    border: "1px solid #27272a",
    borderRadius: "8px",
    background: "#09090b",
    color: "#e4e4e7",
    font: "inherit",
    fontSize: "13px",
    fontWeight: 600,
    cursor: "pointer",
  },
  disabledButton: {
    opacity: 0.45,
    cursor: "not-allowed",
  },
  error: {
    margin: 0,
    padding: "9px 10px",
    border: "1px solid #7f1d1d",
    borderRadius: "8px",
    background: "#2a1113",
    fontSize: "13px",
    lineHeight: 1.5,
    color: "#fca5a5",
  },
  success: {
    margin: 0,
    padding: "9px 10px",
    border: "1px solid #14532d",
    borderRadius: "8px",
    background: "#0d2a1d",
    fontSize: "13px",
    lineHeight: 1.5,
    color: "#86efac",
  },
} satisfies Record<string, CSSProperties>;

export default App;
