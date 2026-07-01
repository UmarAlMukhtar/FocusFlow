import {
  type CSSProperties,
  type PointerEvent,
  useEffect,
  useMemo,
  useState,
} from "react";
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
  preset: ExportPresetKey;
  zoomScale: number;
  zoomInMs: number;
  zoomOutMs: number;
  panTransitionMs: number;
};

type ExportPresetKey = "smallFile" | "balanced" | "highQuality";

type RecordingSourceMode = "screen" | "window" | "region";

type AudioDevice = {
  id: string;
  name: string;
};

type AudioError = {
  code: string;
  message: string;
  recoverable: boolean;
};

type RecordableWindow = {
  id: string;
  title: string;
  x: number;
  y: number;
  width: number;
  height: number;
};

type RegionSelection = {
  x: number;
  y: number;
  width: number;
  height: number;
};

type RegionDraft = {
  startX: number;
  startY: number;
  currentX: number;
  currentY: number;
};

type RecordingSourcePayload =
  | { type: "screen" }
  | { type: "window"; hwnd: string; title: string }
  | { type: "region"; x: number; y: number; width: number; height: number };

type RecentSession = {
  sessionId: string;
  createdAt: string;
  durationSeconds: number;
  recordingSource: string;
  exported: boolean;
  sessionPath: string;
  editedVideoPath: string | null;
};

const EXPORT_PROGRESS_EVENT = "export-progress";
const RECORDING_STATUS_CHANGED_EVENT = "recording-status-changed";
const EXPORT_SETTINGS_STORAGE_KEY = "focusflow.exportSettings";
const MIC_DEVICE_STORAGE_KEY = "focusflow.selectedMicDevice";
const DEFAULT_EXPORT_SETTINGS: ExportSettings = {
  preset: "balanced",
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
  delete_session_failed:
    "FocusFlow could not delete that recording session. Close any open files in the folder and try again.",
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
  invalid_capture_region:
    "Select a recording region that is at least 16 by 16 pixels.",
  invalid_session_id:
    "FocusFlow could not identify that recording session. Refresh the session list and try again.",
  invalid_window_source:
    "The selected window is no longer valid. Refresh the window list and choose again.",
  mic_start_failed:
    "FocusFlow could not start microphone recording. Check microphone access and try again.",
  monitor_query_failed:
    "FocusFlow could not read monitor information for that capture area.",
  non_utf8_path:
    "One of the session paths contains unsupported characters.",
  parse_clicks_failed:
    "FocusFlow could not read the click data for this session.",
  parse_drags_failed:
    "FocusFlow could not read the drag data for this session.",
  parse_session_failed:
    "FocusFlow could not read one of the saved session summaries.",
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
  read_recording_entry_failed:
    "FocusFlow could not read one of the saved recording sessions.",
  read_recording_entry_type_failed:
    "FocusFlow could not read one of the saved recording folders.",
  read_recordings_dir_failed:
    "FocusFlow could not read the recordings folder.",
  read_session_failed:
    "FocusFlow could not open one of the saved session summaries.",
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
  resolve_recordings_dir_failed:
    "FocusFlow could not verify the recordings folder. Restart the app and try again.",
  resolve_session_dir_failed:
    "That recording session folder is no longer available.",
  serialize_session_failed:
    "FocusFlow could not prepare the session summary.",
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
  write_metadata_failed:
    "FocusFlow could not save recording metadata for this session.",
  write_session_failed:
    "FocusFlow could not save the session summary.",
  write_timeline_failed:
    "FocusFlow could not save the zoom timeline for this recording.",
  window_enumeration_failed:
    "FocusFlow could not read the list of available windows.",
  window_source_unavailable:
    "The selected window is no longer available. Refresh the list and choose again.",
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
  const [exportSettings] = useState<ExportSettings>(() =>
    loadExportSettings(),
  );
  const [recentSessions, setRecentSessions] = useState<RecentSession[]>([]);
  const [selectedSessionId, setSelectedSessionId] = useState("");
  const [isLoadingSessions, setIsLoadingSessions] = useState(false);
  const [recordingSourceMode, setRecordingSourceMode] =
    useState<RecordingSourceMode>("screen");
  const [recordableWindows, setRecordableWindows] = useState<
    RecordableWindow[]
  >([]);
  const [selectedWindowId, setSelectedWindowId] = useState("");
  const [isLoadingWindows, setIsLoadingWindows] = useState(false);
  const [selectedRegion, setSelectedRegion] =
    useState<RegionSelection | null>(null);
  const [isSelectingRegion, setIsSelectingRegion] = useState(false);
  const [regionDraft, setRegionDraft] = useState<RegionDraft | null>(null);
  const [regionWindowOffset, setRegionWindowOffset] = useState({ x: 0, y: 0 });
  const [audioDevices, setAudioDevices] = useState<AudioDevice[]>([]);
  const [isLoadingDevices, setIsLoadingDevices] = useState(false);
  const [micError, setMicError] = useState<string | null>(null);
  const [micEnabled, setMicEnabled] = useState(false);
  const [selectedDeviceId, setSelectedDeviceId] = useState(() =>
    loadSelectedMicDevice(),
  );

  const statusLabel = useMemo(() => {
    if (countdown !== null) return "Starting...";
    if (isExporting) return "Exporting...";
    if (isBusy) return "Working...";
    if (!status) return "Unknown";
    return formatPhase(status.phase);
  }, [countdown, isBusy, isExporting, status]);

  const isRecording = status?.phase === "recording";
  const isFinalizing = status?.phase === "finalizing";
  const micControlsDisabled = isBusy || isRecording || isFinalizing;
  const elapsedLabel = formatElapsedTime(status?.elapsedMs ?? 0);
  const currentSessionPath = useMemo(
    () => sessionPathFromOutput(status?.outputPath ?? ""),
    [status?.outputPath],
  );
  const editedVideoFolderPath = useMemo(
    () => sessionPathFromOutput(exportOutputPath ?? ""),
    [exportOutputPath],
  );
  const activeSession = useMemo(() => {
    if (selectedSessionId) {
      const selectedSession = recentSessions.find(
        (session) => session.sessionId === selectedSessionId,
      );

      if (selectedSession) {
        return selectedSession;
      }
    }

    const currentPath = currentSessionPath || editedVideoFolderPath;

    if (currentPath) {
      const matchingSession = recentSessions.find(
        (session) => session.sessionPath === currentPath,
      );

      if (matchingSession) {
        return matchingSession;
      }
    }

    return recentSessions[0] ?? null;
  }, [
    currentSessionPath,
    editedVideoFolderPath,
    recentSessions,
    selectedSessionId,
  ]);
  const activeSessionFolderPath =
    activeSession?.sessionPath ?? currentSessionPath;
  const activeEditedVideoFolderPath =
    activeSession?.editedVideoPath !== null && activeSession?.editedVideoPath
      ? sessionPathFromOutput(activeSession.editedVideoPath)
      : editedVideoFolderPath;
  const selectedWindow = useMemo(
    () =>
      recordableWindows.find((window) => window.id === selectedWindowId) ??
      null,
    [recordableWindows, selectedWindowId],
  );
  const recordingSourcePayload = useMemo(
    () =>
      buildRecordingSourcePayload(
        recordingSourceMode,
        selectedWindow,
        selectedRegion,
      ),
    [recordingSourceMode, selectedRegion, selectedWindow],
  );
  const recordingSourceReady = recordingSourcePayload !== null;
  const startDisabledReason = recordingSourceReady
    ? undefined
    : sourceValidationMessage(recordingSourceMode);

  useEffect(() => {
    void refreshStatus();
    void refreshRecentSessions(false);
  }, []);

  useEffect(() => {
    if (!isSelectingRegion) return;

    function handleRegionKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") {
        event.preventDefault();
        void cancelRegionSelection();
        return;
      }

      if (event.key === "Enter") {
        event.preventDefault();
        void confirmRegionSelection();
      }
    }

    window.addEventListener("keydown", handleRegionKeyDown);

    return () => {
      window.removeEventListener("keydown", handleRegionKeyDown);
    };
  }, [isSelectingRegion, selectedRegion]);

  useEffect(() => {
    if (recordingSourceMode === "window" && recordableWindows.length === 0) {
      void refreshRecordableWindows();
    }
  }, [recordableWindows.length, recordingSourceMode]);

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

  useEffect(() => {
    saveSelectedMicDevice(selectedDeviceId);
  }, [selectedDeviceId]);

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

  async function refreshRecentSessions(clearError = true) {
    setIsLoadingSessions(true);

    try {
      const sessions = await invoke<RecentSession[]>("list_recent_sessions");
      setRecentSessions(sessions);
      if (clearError) {
        setError(null);
      }
    } catch (caught) {
      setError(formatError(caught));
    } finally {
      setIsLoadingSessions(false);
    }
  }

  async function refreshRecordableWindows() {
    setIsLoadingWindows(true);
    setError(null);

    try {
      const windows = await invoke<RecordableWindow[]>("list_recordable_windows");
      setRecordableWindows(windows);
      setSelectedWindowId((current) =>
        windows.some((window) => window.id === current) ? current : "",
      );
    } catch (caught) {
      setError(formatError(caught));
    } finally {
      setIsLoadingWindows(false);
    }
  }

  function updateRecordingSourceMode(nextMode: RecordingSourceMode) {
    setRecordingSourceMode(nextMode);
    setError(null);
  }

  async function beginRegionSelection() {
    setError(null);
    setRegionDraft(null);
    setIsSelectingRegion(true);

    try {
      const appWindow = getCurrentWindow();
      await appWindow.setFullscreen(true);
      await wait(120);
      const position = await appWindow.outerPosition();
      setRegionWindowOffset({ x: position.x, y: position.y });
    } catch (caught) {
      setIsSelectingRegion(false);
      setRegionDraft(null);
      await exitFullscreenSelection();
      setError(`Region selection could not start: ${formatError(caught)}`);
    }
  }

  async function confirmRegionSelection() {
    if (!selectedRegion) return;

    setIsSelectingRegion(false);
    setRegionDraft(null);
    await exitFullscreenSelection();
  }

  async function cancelRegionSelection() {
    setIsSelectingRegion(false);
    setRegionDraft(null);
    setSelectedRegion(null);
    await exitFullscreenSelection();
  }

  function handleRegionPointerDown(event: PointerEvent<HTMLDivElement>) {
    if (event.button !== 0) return;

    event.currentTarget.setPointerCapture(event.pointerId);
    setSelectedRegion(null);
    setRegionDraft({
      startX: event.clientX,
      startY: event.clientY,
      currentX: event.clientX,
      currentY: event.clientY,
    });
  }

  function handleRegionPointerMove(event: PointerEvent<HTMLDivElement>) {
    setRegionDraft((current) =>
      current
        ? {
            ...current,
            currentX: event.clientX,
            currentY: event.clientY,
          }
        : current,
    );
  }

  function handleRegionPointerUp(event: PointerEvent<HTMLDivElement>) {
    setRegionDraft((current) => {
      if (!current) return current;

      const draft = {
        ...current,
        currentX: event.clientX,
        currentY: event.clientY,
      };
      const nextRegion = physicalRegionFromDraft(draft, regionWindowOffset);

      if (nextRegion.width < 16 || nextRegion.height < 16) {
        setSelectedRegion(null);
        setError("Select a region that is at least 16 by 16 pixels.");
      } else {
        setSelectedRegion(nextRegion);
        setError(null);
      }

      return draft;
    });
  }

  async function startRecording() {
    if (!recordingSourcePayload) {
      setError(sourceValidationMessage(recordingSourceMode));
      return;
    }

    if (micEnabled && !selectedDeviceId) {
      setMicError("Choose a microphone before starting, or turn microphone off.");
      return;
    }

    if (micEnabled && isLoadingDevices) {
      setMicError("Wait for the microphone list to finish loading.");
      return;
    }

    if (micEnabled && audioDevices.length === 0) {
      setMicError("No microphone is available. Connect one or turn microphone off.");
      return;
    }

    if (
      micEnabled &&
      !audioDevices.some((device) => device.id === selectedDeviceId)
    ) {
      setMicError("Selected microphone is unavailable. Refresh devices or choose another mic.");
      return;
    }

    setIsBusy(true);
    setCountdown(null);
    setError(null);
    setMicError(null);
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

      const nextStatus = await invoke<RecordingStatus>("start_recording", {
        source: recordingSourcePayload,
        audioDeviceId: micEnabled ? (selectedDeviceId || null) : null,
      });
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
      await refreshRecentSessions(false);
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
      await refreshRecentSessions(false);
    } catch (caught) {
      setError(formatError(caught));
      setExportProgressPercentage(null);
    } finally {
      setIsExporting(false);
      setIsBusy(false);
    }
  }

  async function deleteSession(session: RecentSession) {
    const confirmed = window.confirm(
      `Delete recording session ${session.sessionId}?`,
    );

    if (!confirmed) return;

    setIsBusy(true);
    setError(null);

    try {
      await invoke("delete_recording_session", {
        sessionId: session.sessionId,
      });
      if (currentSessionPath === session.sessionPath) {
        setStatus(null);
      }
      if (selectedSessionId === session.sessionId) {
        setSelectedSessionId("");
      }
      if (editedVideoFolderPath === session.sessionPath) {
        setExportOutputPath(null);
        setExportMessage(null);
      }
      await refreshRecentSessions(false);
    } catch (caught) {
      setError(formatError(caught));
    } finally {
      setIsBusy(false);
    }
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

  async function refreshAudioDevices() {
    setIsLoadingDevices(true);
    setMicError(null);
    try {
      const devices = await invoke<AudioDevice[]>("list_audio_input_devices");
      setAudioDevices(devices);
      if (devices.length > 0) {
        setSelectedDeviceId((prev) => {
          if (prev && devices.some((device) => device.id === prev)) {
            return prev;
          }
          return devices[0].id;
        });
      } else {
        setSelectedDeviceId("");
        setMicError("No microphone is available. Connect one or turn microphone off.");
      }
    } catch (caught) {
      const err = caught as AudioError;
      const msg = err?.message ?? String(caught);
      setMicError(msg);
    } finally {
      setIsLoadingDevices(false);
    }
  }

  async function handleMicToggle(enabled: boolean) {
    setMicEnabled(enabled);
    if (enabled) {
      await refreshAudioDevices();
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

        <div style={styles.workspace}>
          <div style={styles.leftColumn}>
            <section style={styles.card}>
              <div style={styles.cardHeader}>
                <div>
                  <p style={styles.label}>Status</p>
                  <p style={styles.cardTitle}>Recording overview</p>
                </div>
              </div>

              <div style={styles.statusPanel}>
                <div style={styles.statTile}>
                  <p style={styles.label}>State</p>
                  <p style={styles.statusValue}>{statusLabel}</p>
                </div>
                <div style={styles.statTile}>
                  <p style={styles.label}>Elapsed</p>
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
                  <p style={styles.label}>Recorder</p>
                  <p style={styles.cardTitle}>Capture source</p>
                </div>
              </div>

              <div style={styles.sourceSection}>
                <div style={styles.sourceModeGrid}>
                  {(
                    ["screen", "window", "region"] as RecordingSourceMode[]
                  ).map((mode) => (
                    <button
                      key={mode}
                      type="button"
                      onClick={() => updateRecordingSourceMode(mode)}
                      disabled={isBusy || isRecording}
                      style={{
                        ...styles.sourceModeButton,
                        ...(recordingSourceMode === mode
                          ? styles.sourceModeButtonActive
                          : {}),
                        ...(isBusy || isRecording
                          ? styles.disabledButton
                          : {}),
                      }}
                    >
                      {sourceModeLabel(mode)}
                    </button>
                  ))}
                </div>

                {recordingSourceMode === "window" ? (
                  <div style={styles.sourceDetails}>
                    <select
                      value={selectedWindowId}
                      onChange={(event) =>
                        setSelectedWindowId(event.currentTarget.value)
                      }
                      disabled={isBusy || isRecording || isLoadingWindows}
                      style={styles.selectInput}
                    >
                      <option value="">Choose window...</option>
                      {recordableWindows.map((window) => (
                        <option key={window.id} value={window.id}>
                          {window.title}
                        </option>
                      ))}
                    </select>
                    <button
                      type="button"
                      onClick={() => void refreshRecordableWindows()}
                      disabled={isBusy || isRecording || isLoadingWindows}
                      style={{
                        ...styles.ghostButton,
                        ...(isBusy || isRecording || isLoadingWindows
                          ? styles.disabledButton
                          : {}),
                      }}
                    >
                      {isLoadingWindows ? "Loading" : "Refresh"}
                    </button>
                  </div>
                ) : null}

                {recordingSourceMode === "region" ? (
                  <div style={styles.sourceDetails}>
                    <p style={styles.sourceSummary}>
                      {selectedRegion
                        ? formatRegionSelection(selectedRegion)
                        : "No region selected"}
                    </p>
                    <button
                      type="button"
                      onClick={() => void beginRegionSelection()}
                      disabled={isBusy || isRecording}
                      style={{
                        ...styles.ghostButton,
                        ...(isBusy || isRecording ? styles.disabledButton : {}),
                      }}
                    >
                      Select Region
                    </button>
                  </div>
                ) : null}

                {!recordingSourceReady ? (
                  <p style={styles.sourceHint}>
                    {sourceValidationMessage(recordingSourceMode)}
                  </p>
                ) : null}
              </div>

              <div style={styles.controls}>
                <button
                  type="button"
                  onClick={startRecording}
                  disabled={isBusy || isRecording || !recordingSourceReady}
                  title={startDisabledReason}
                  style={{
                    ...styles.startButton,
                    ...(isBusy || isRecording || !recordingSourceReady
                      ? styles.disabledButton
                      : {}),
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
                  <p style={styles.label}>Microphone</p>
                  <p style={styles.cardTitle}>Audio recording</p>
                </div>
              </div>

              <div style={{ display: "flex", flexDirection: "column", gap: "12px" }}>
                <label style={{ display: "flex", alignItems: "center", gap: "10px", cursor: "pointer", fontSize: "14px", color: "#fafafa" }}>
                  <input
                    type="checkbox"
                    checked={micEnabled}
                    onChange={(e) => void handleMicToggle(e.target.checked)}
                    disabled={micControlsDisabled}
                    style={{
                      cursor: micControlsDisabled ? "not-allowed" : "pointer",
                    }}
                  />
                  Enable Microphone
                </label>

                {micEnabled && (
                  <div style={{ display: "flex", flexDirection: "column", gap: "6px" }}>
                    <label style={{ fontSize: "12px", color: "#a1a1aa" }}>Input Device</label>
                    <div style={{ display: "flex", gap: "8px" }}>
                      <select
                        value={selectedDeviceId}
                        onChange={(e) => setSelectedDeviceId(e.target.value)}
                        disabled={micControlsDisabled || isLoadingDevices}
                        style={{
                          flex: 1,
                          background: "#1c1c20",
                          border: "1px solid #27272a",
                          color: "#fafafa",
                          borderRadius: "6px",
                          padding: "6px 10px",
                          fontSize: "13px",
                          outline: "none",
                        }}
                      >
                        {audioDevices.length === 0 ? (
                          <option value="">No microphone devices found</option>
                        ) : (
                          audioDevices.map((device) => (
                            <option key={device.id} value={device.id}>
                              {device.name}
                            </option>
                          ))
                        )}
                      </select>
                      <button
                        type="button"
                        onClick={() => void refreshAudioDevices()}
                        disabled={micControlsDisabled || isLoadingDevices}
                        style={{
                          ...styles.ghostButton,
                          padding: "6px 12px",
                          ...(micControlsDisabled || isLoadingDevices
                            ? styles.disabledButton
                            : {}),
                        }}
                      >
                        Refresh
                      </button>
                    </div>
                  </div>
                )}

                {micError ? (
                  <p style={{ ...styles.error, margin: 0 }}>
                    {micError}
                  </p>
                ) : null}
              </div>
            </section>

            {exportMessage ? (
              <p style={styles.success}>{exportMessage}</p>
            ) : null}

            {error ? <p style={styles.error}>{error}</p> : null}
          </div>

          <aside style={styles.rightColumn}>
            <section style={styles.card}>
              <div style={styles.cardHeader}>
                <div>
                  <p style={styles.label}>Recent sessions</p>
                  <p style={styles.cardTitle}>Saved recordings</p>
                </div>
              </div>

              <div style={styles.sessionList}>
                {recentSessions.length === 0 ? (
                  <p style={styles.emptyState}>
                    {isLoadingSessions ? "Loading sessions..." : "No sessions"}
                  </p>
                ) : (
                  recentSessions.map((session) => (
                    <button
                      key={session.sessionId}
                      type="button"
                      onClick={() => setSelectedSessionId(session.sessionId)}
                      style={{
                        ...styles.sessionRow,
                        ...(activeSession?.sessionId === session.sessionId
                          ? styles.sessionRowActive
                          : {}),
                      }}
                    >
                      <div style={styles.sessionMeta}>
                        <div style={styles.sessionTitleRow}>
                          <span style={styles.sessionId}>
                            {session.sessionId}
                          </span>
                          <span
                            style={{
                              ...styles.exportBadge,
                              ...(session.exported
                                ? styles.exportBadgeReady
                                : {}),
                            }}
                          >
                            {session.exported ? "Exported" : "Draft"}
                          </span>
                        </div>
                        <p style={styles.sessionDetail}>
                          {formatSessionDate(session.createdAt)} -{" "}
                          {formatSessionDuration(session.durationSeconds)}
                        </p>
                      </div>
                    </button>
                  ))
                )}
              </div>
            </section>

            <section style={styles.card}>
              <div style={styles.cardHeader}>
                <div>
                  <p style={styles.label}>Metadata</p>
                  <p style={styles.cardTitle}>Current session</p>
                </div>
              </div>

              {activeSession ? (
                <div style={styles.metadataGrid}>
                  <div style={styles.metadataRow}>
                    <span style={styles.metadataLabel}>ID</span>
                    <span style={styles.metadataValue}>
                      {activeSession.sessionId}
                    </span>
                  </div>
                  <div style={styles.metadataRow}>
                    <span style={styles.metadataLabel}>Date</span>
                    <span style={styles.metadataValue}>
                      {formatSessionDate(activeSession.createdAt)}
                    </span>
                  </div>
                  <div style={styles.metadataRow}>
                    <span style={styles.metadataLabel}>Duration</span>
                    <span style={styles.metadataValue}>
                      {formatSessionDuration(activeSession.durationSeconds)}
                    </span>
                  </div>
                  <div style={styles.metadataRow}>
                    <span style={styles.metadataLabel}>Source</span>
                    <span style={styles.metadataValue}>
                      {sourceModeLabelFromMetadata(
                        activeSession.recordingSource,
                      )}
                    </span>
                  </div>
                  <div style={styles.metadataRow}>
                    <span style={styles.metadataLabel}>Export</span>
                    <span style={styles.metadataValue}>
                      {activeSession.exported ? "Ready" : "Pending"}
                    </span>
                  </div>
                </div>
              ) : (
                <p style={styles.emptyState}>No session selected.</p>
              )}
            </section>

            <section style={styles.card}>
              <div style={styles.cardHeader}>
                <div>
                  <p style={styles.label}>Actions</p>
                  <p style={styles.cardTitle}>Session folders</p>
                </div>
              </div>

              <div style={styles.folderActions}>
                <button
                  type="button"
                  onClick={() =>
                    void openFolder(
                      "open_recordings_folder",
                      "Recordings folder",
                    )
                  }
                  style={styles.folderButton}
                >
                  Recordings
                </button>

                <button
                  type="button"
                  onClick={() =>
                    void openFolder(
                      "open_recording_session_folder",
                      "Session folder",
                      activeSessionFolderPath || null,
                    )
                  }
                  style={styles.folderButton}
                >
                  Session
                </button>

                <button
                  type="button"
                  onClick={() =>
                    void openFolder(
                      "open_edited_video_folder",
                      "Edited video folder",
                      activeEditedVideoFolderPath,
                    )
                  }
                  disabled={!activeEditedVideoFolderPath}
                  style={{
                    ...styles.folderButton,
                    ...(!activeEditedVideoFolderPath
                      ? styles.disabledButton
                      : {}),
                  }}
                >
                  Edited
                </button>

                {activeSession ? (
                  <button
                    type="button"
                    onClick={() => void deleteSession(activeSession)}
                    disabled={isBusy}
                    style={{
                      ...styles.dangerFolderButton,
                      ...(isBusy ? styles.disabledButton : {}),
                    }}
                  >
                    Delete
                  </button>
                ) : null}
              </div>
            </section>
          </aside>
        </div>
      </section>

      {countdown !== null ? (
        <div aria-live="assertive" style={styles.countdownOverlay}>
          <span style={styles.countdownValue}>{countdown}</span>
        </div>
      ) : null}

      {isSelectingRegion ? (
        <div
          role="presentation"
          style={styles.regionOverlay}
          onPointerDown={handleRegionPointerDown}
          onPointerMove={handleRegionPointerMove}
          onPointerUp={handleRegionPointerUp}
        >
          <div
            style={styles.regionToolbar}
            onPointerDown={(event) => event.stopPropagation()}
            onPointerMove={(event) => event.stopPropagation()}
            onPointerUp={(event) => event.stopPropagation()}
          >
            <div>
              <p style={styles.regionToolbarTitle}>Select recording region</p>
              <p style={styles.regionToolbarText}>
                Drag to draw the capture area. Enter confirms, Esc cancels.
              </p>
            </div>
            <div style={styles.regionToolbarActions}>
              <button
                type="button"
                onPointerDown={(event) => event.stopPropagation()}
                onClick={() => void cancelRegionSelection()}
                style={styles.secondaryButton}
              >
                Cancel
              </button>
              <button
                type="button"
                onPointerDown={(event) => event.stopPropagation()}
                onClick={() => void confirmRegionSelection()}
                disabled={!selectedRegion}
                style={{
                  ...styles.startButton,
                  ...(!selectedRegion ? styles.disabledButton : {}),
                }}
              >
                Confirm
              </button>
            </div>
          </div>

          {regionDraft ? (
            <>
              <div style={regionDraftBoxStyle(regionDraft)} />
              <div style={regionDraftBadgeStyle(regionDraft)}>
                {formatRegionDraftDimensions(regionDraft)}
              </div>
            </>
          ) : null}
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

function buildRecordingSourcePayload(
  mode: RecordingSourceMode,
  selectedWindow: RecordableWindow | null,
  selectedRegion: RegionSelection | null,
): RecordingSourcePayload | null {
  if (mode === "screen") {
    return { type: "screen" };
  }

  if (mode === "window") {
    if (!selectedWindow) return null;

    return {
      type: "window",
      hwnd: selectedWindow.id,
      title: selectedWindow.title,
    };
  }

  if (!selectedRegion) return null;

  return {
    type: "region",
    x: selectedRegion.x,
    y: selectedRegion.y,
    width: selectedRegion.width,
    height: selectedRegion.height,
  };
}

function sourceModeLabel(mode: RecordingSourceMode) {
  switch (mode) {
    case "screen":
      return "Entire Screen";
    case "window":
      return "Window";
    case "region":
      return "Region";
  }
}

function sourceModeLabelFromMetadata(source: string) {
  switch (source) {
    case "screen":
      return "Entire Screen";
    case "window":
      return "Window";
    case "region":
      return "Region";
    default:
      return "Unknown source";
  }
}

function sourceValidationMessage(mode: RecordingSourceMode) {
  switch (mode) {
    case "screen":
      return "";
    case "window":
      return "Choose a window before starting.";
    case "region":
      return "Select a region before starting.";
  }
}

function formatRegionSelection(region: RegionSelection) {
  return `${region.width} x ${region.height} at ${region.x}, ${region.y}`;
}

function formatRegionDraftDimensions(draft: RegionDraft) {
  const region = logicalRegionFromDraft(draft);
  const scale = window.devicePixelRatio || 1;
  const width = Math.round(region.width * scale);
  const height = Math.round(region.height * scale);

  return `${width} x ${height}`;
}

function physicalRegionFromDraft(
  draft: RegionDraft,
  windowOffset: { x: number; y: number },
): RegionSelection {
  const logicalRegion = logicalRegionFromDraft(draft);
  const scale = window.devicePixelRatio || 1;

  return {
    x: Math.round(windowOffset.x + logicalRegion.x * scale),
    y: Math.round(windowOffset.y + logicalRegion.y * scale),
    width: Math.round(logicalRegion.width * scale),
    height: Math.round(logicalRegion.height * scale),
  };
}

function logicalRegionFromDraft(draft: RegionDraft) {
  const x = Math.min(draft.startX, draft.currentX);
  const y = Math.min(draft.startY, draft.currentY);
  const width = Math.abs(draft.currentX - draft.startX);
  const height = Math.abs(draft.currentY - draft.startY);

  return { x, y, width, height };
}

function regionDraftBoxStyle(draft: RegionDraft): CSSProperties {
  const region = logicalRegionFromDraft(draft);

  return {
    ...styles.regionSelectionBox,
    left: `${region.x}px`,
    top: `${region.y}px`,
    width: `${region.width}px`,
    height: `${region.height}px`,
  };
}

function regionDraftBadgeStyle(draft: RegionDraft): CSSProperties {
  const region = logicalRegionFromDraft(draft);
  const left = Math.min(region.x + region.width + 12, window.innerWidth - 132);
  const top = Math.max(12, region.y - 38);

  return {
    ...styles.regionDimensionBadge,
    left: `${left}px`,
    top: `${top}px`,
  };
}

function formatSessionDate(value: string) {
  const date = new Date(value);

  if (Number.isNaN(date.getTime())) {
    return value || "Unknown date";
  }

  return date.toLocaleString([], {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

function formatSessionDuration(seconds: number) {
  if (!Number.isFinite(seconds) || seconds <= 0) {
    return "00:00";
  }

  const totalSeconds = Math.floor(seconds);
  const hours = Math.floor(totalSeconds / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  const remainingSeconds = totalSeconds % 60;

  if (hours > 0) {
    return [hours, minutes, remainingSeconds]
      .map((part) => part.toString().padStart(2, "0"))
      .join(":");
  }

  return [minutes, remainingSeconds]
    .map((part) => part.toString().padStart(2, "0"))
    .join(":");
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

function loadSelectedMicDevice() {
  try {
    return window.localStorage.getItem(MIC_DEVICE_STORAGE_KEY) ?? "";
  } catch {
    return "";
  }
}

function saveSelectedMicDevice(deviceId: string) {
  try {
    if (deviceId) {
      window.localStorage.setItem(MIC_DEVICE_STORAGE_KEY, deviceId);
    } else {
      window.localStorage.removeItem(MIC_DEVICE_STORAGE_KEY);
    }
  } catch {
    // Ignore storage failures; mic selection still works in memory.
  }
}

function normalizeExportSettings(value: unknown): ExportSettings {
  if (!value || typeof value !== "object") {
    return DEFAULT_EXPORT_SETTINGS;
  }

  const settings = value as Partial<Record<keyof ExportSettings, unknown>>;

  return {
    preset: normalizeExportPreset(settings.preset),
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

function normalizeExportPreset(value: unknown): ExportPresetKey {
  if (
    value === "smallFile" ||
    value === "balanced" ||
    value === "highQuality"
  ) {
    return value;
  }

  return DEFAULT_EXPORT_SETTINGS.preset;
}

function positiveNumberOrDefault(value: unknown, fallback: number) {
  return typeof value === "number" && Number.isFinite(value) && value > 0
    ? value
    : fallback;
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

async function exitFullscreenSelection() {
  try {
    await getCurrentWindow().setFullscreen(false);
  } catch (caught) {
    console.error("Could not exit region selection fullscreen", caught);
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
    width: "min(1120px, 100%)",
    height: "100%",
    maxHeight: "100%",
    boxSizing: "border-box",
    display: "grid",
    gridTemplateRows: "auto minmax(0, 1fr)",
    gap: "10px",
    overflow: "hidden",
    padding: "14px",
    border: "1px solid #27272a",
    borderRadius: "14px",
    background: "#0f1014",
    boxShadow: "0 24px 64px rgba(0, 0, 0, 0.42)",
  },
  workspace: {
    minHeight: 0,
    display: "grid",
    gridTemplateColumns: "minmax(0, 7fr) minmax(224px, 3fr)",
    gap: "10px",
    overflow: "hidden",
  },
  leftColumn: {
    minWidth: 0,
    minHeight: 0,
    display: "grid",
    alignContent: "start",
    gap: "10px",
    overflow: "hidden",
  },
  rightColumn: {
    minWidth: 0,
    minHeight: 0,
    display: "grid",
    gridTemplateRows: "minmax(0, 1fr) auto auto",
    gap: "10px",
    overflow: "hidden",
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
    gridTemplateColumns: "repeat(2, minmax(0, 1fr))",
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
  presetGrid: {
    display: "grid",
    gridTemplateColumns: "repeat(3, minmax(0, 1fr))",
    gap: "8px",
    marginBottom: "10px",
  },
  presetButton: {
    minHeight: "52px",
    display: "grid",
    alignContent: "center",
    gap: "3px",
    padding: "8px 10px",
    border: "1px solid #27272a",
    borderRadius: "8px",
    background: "#09090b",
    color: "#e4e4e7",
    font: "inherit",
    textAlign: "left",
    cursor: "pointer",
  },
  presetButtonActive: {
    borderColor: "#86efac",
    background: "#0d2a1d",
    color: "#bbf7d0",
  },
  presetLabel: {
    fontSize: "12px",
    fontWeight: 700,
    lineHeight: 1.2,
  },
  presetDetail: {
    color: "#a1a1aa",
    fontSize: "11px",
    fontWeight: 500,
    lineHeight: 1.25,
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
  sourceSection: {
    display: "grid",
    gap: "8px",
    marginBottom: "8px",
  },
  sourceModeGrid: {
    display: "grid",
    gridTemplateColumns: "repeat(3, minmax(0, 1fr))",
    gap: "6px",
  },
  sourceModeButton: {
    minHeight: "34px",
    padding: "0 10px",
    border: "1px solid #27272a",
    borderRadius: "8px",
    background: "#09090b",
    color: "#e4e4e7",
    font: "inherit",
    fontSize: "12px",
    fontWeight: 650,
    cursor: "pointer",
  },
  sourceModeButtonActive: {
    borderColor: "#86efac",
    background: "#0d2a1d",
    color: "#bbf7d0",
  },
  sourceDetails: {
    display: "grid",
    gridTemplateColumns: "minmax(0, 1fr) auto",
    gap: "8px",
    alignItems: "center",
  },
  sourceSummary: {
    minHeight: "34px",
    display: "flex",
    alignItems: "center",
    margin: 0,
    padding: "0 10px",
    border: "1px solid #27272a",
    borderRadius: "8px",
    background: "#09090b",
    color: "#d4d4d8",
    fontSize: "12px",
    lineHeight: 1.35,
    overflow: "hidden",
    textOverflow: "ellipsis",
    whiteSpace: "nowrap",
  },
  sourceHint: {
    margin: 0,
    color: "#fbbf24",
    fontSize: "12px",
    lineHeight: 1.35,
  },
  selectInput: {
    width: "100%",
    minHeight: "34px",
    minWidth: 0,
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
  folderActions: {
    display: "grid",
    gridTemplateColumns: "repeat(2, minmax(0, 1fr))",
    gap: "6px",
  },
  sessionList: {
    display: "grid",
    alignContent: "start",
    gap: "6px",
    minHeight: 0,
    maxHeight: "100%",
    overflowX: "hidden",
    overflowY: "auto",
    paddingRight: "2px",
  },
  emptyState: {
    margin: 0,
    padding: "10px",
    border: "1px dashed #27272a",
    borderRadius: "8px",
    color: "#a1a1aa",
    fontSize: "13px",
    lineHeight: 1.4,
  },
  sessionRow: {
    display: "grid",
    width: "100%",
    gridTemplateColumns: "minmax(0, 1fr)",
    gap: "6px",
    alignItems: "center",
    minWidth: 0,
    margin: 0,
    padding: "8px",
    border: "1px solid #27272a",
    borderRadius: "8px",
    background: "#0f1014",
    color: "inherit",
    font: "inherit",
    textAlign: "left",
    cursor: "pointer",
  },
  sessionRowActive: {
    borderColor: "#86efac",
    background: "#102016",
  },
  sessionMeta: {
    display: "grid",
    gap: "4px",
    minWidth: 0,
  },
  sessionTitleRow: {
    display: "flex",
    alignItems: "center",
    gap: "8px",
    minWidth: 0,
  },
  sessionId: {
    minWidth: 0,
    overflow: "hidden",
    color: "#fafafa",
    fontSize: "13px",
    fontWeight: 700,
    lineHeight: 1.2,
    textOverflow: "ellipsis",
    whiteSpace: "nowrap",
  },
  sessionDetail: {
    margin: 0,
    overflow: "hidden",
    color: "#a1a1aa",
    fontSize: "12px",
    lineHeight: 1.35,
    textOverflow: "ellipsis",
    whiteSpace: "nowrap",
  },
  exportBadge: {
    flex: "0 0 auto",
    minHeight: "22px",
    display: "inline-flex",
    alignItems: "center",
    padding: "0 8px",
    border: "1px solid #3f3f46",
    borderRadius: "999px",
    background: "#18181b",
    color: "#d4d4d8",
    fontSize: "11px",
    fontWeight: 700,
  },
  exportBadgeReady: {
    borderColor: "#14532d",
    background: "#0d2a1d",
    color: "#86efac",
  },
  sessionActions: {
    display: "flex",
    alignItems: "center",
    gap: "6px",
  },
  miniButton: {
    minHeight: "30px",
    padding: "0 9px",
    border: "1px solid #27272a",
    borderRadius: "8px",
    background: "#09090b",
    color: "#e4e4e7",
    font: "inherit",
    fontSize: "12px",
    fontWeight: 650,
    cursor: "pointer",
  },
  dangerMiniButton: {
    minHeight: "30px",
    padding: "0 9px",
    border: "1px solid #7f1d1d",
    borderRadius: "8px",
    background: "#2a1113",
    color: "#fecaca",
    font: "inherit",
    fontSize: "12px",
    fontWeight: 650,
    cursor: "pointer",
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
    minHeight: "32px",
    padding: "0 10px",
    border: "1px solid #27272a",
    borderRadius: "8px",
    background: "#09090b",
    color: "#e4e4e7",
    font: "inherit",
    fontSize: "12px",
    fontWeight: 600,
    cursor: "pointer",
  },
  dangerFolderButton: {
    minHeight: "32px",
    padding: "0 10px",
    border: "1px solid #7f1d1d",
    borderRadius: "8px",
    background: "#2a1113",
    color: "#fecaca",
    font: "inherit",
    fontSize: "12px",
    fontWeight: 650,
    cursor: "pointer",
  },
  metadataGrid: {
    display: "grid",
    gap: "6px",
  },
  metadataRow: {
    display: "grid",
    gridTemplateColumns: "70px minmax(0, 1fr)",
    gap: "8px",
    alignItems: "center",
    minHeight: "24px",
  },
  metadataLabel: {
    color: "#71717a",
    fontSize: "10px",
    fontWeight: 700,
    lineHeight: 1.2,
    textTransform: "uppercase",
  },
  metadataValue: {
    minWidth: 0,
    overflow: "hidden",
    color: "#e4e4e7",
    fontSize: "12px",
    fontWeight: 600,
    lineHeight: 1.3,
    textOverflow: "ellipsis",
    whiteSpace: "nowrap",
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
  regionOverlay: {
    position: "fixed",
    inset: 0,
    zIndex: 20,
    overflow: "hidden",
    background: "rgba(2, 6, 23, 0.82)",
    cursor: "crosshair",
    userSelect: "none",
  },
  regionToolbar: {
    position: "absolute",
    top: "16px",
    left: "50%",
    zIndex: 22,
    width: "min(680px, calc(100vw - 32px))",
    minHeight: "58px",
    transform: "translateX(-50%)",
    display: "flex",
    alignItems: "center",
    justifyContent: "space-between",
    gap: "16px",
    padding: "10px 12px",
    border: "1px solid #27272a",
    borderRadius: "10px",
    background: "rgba(9, 9, 11, 0.92)",
    boxShadow: "0 18px 48px rgba(0, 0, 0, 0.36)",
    cursor: "default",
  },
  regionToolbarTitle: {
    margin: 0,
    color: "#fafafa",
    fontSize: "14px",
    fontWeight: 700,
    lineHeight: 1.2,
  },
  regionToolbarText: {
    margin: "4px 0 0",
    color: "#a1a1aa",
    fontSize: "12px",
    lineHeight: 1.35,
  },
  regionToolbarActions: {
    display: "flex",
    alignItems: "center",
    gap: "8px",
  },
  regionSelectionBox: {
    position: "absolute",
    zIndex: 21,
    border: "2px solid #86efac",
    borderRadius: "4px",
    background: "rgba(134, 239, 172, 0.12)",
    boxShadow:
      "0 0 0 9999px rgba(2, 6, 23, 0.52), 0 0 0 1px rgba(9, 9, 11, 0.85) inset",
    pointerEvents: "none",
  },
  regionDimensionBadge: {
    position: "absolute",
    zIndex: 23,
    minWidth: "112px",
    minHeight: "28px",
    display: "inline-flex",
    alignItems: "center",
    justifyContent: "center",
    padding: "0 9px",
    border: "1px solid #27272a",
    borderRadius: "999px",
    background: "rgba(9, 9, 11, 0.92)",
    color: "#fafafa",
    fontSize: "12px",
    fontWeight: 700,
    fontVariantNumeric: "tabular-nums",
    pointerEvents: "none",
  },
} satisfies Record<string, CSSProperties>;

export default App;
