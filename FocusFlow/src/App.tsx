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

function App() {
  const [status, setStatus] = useState<RecordingStatus | null>(null);
  const [isBusy, setIsBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [exportMessage, setExportMessage] = useState<string | null>(null);
  const [exportOutputPath, setExportOutputPath] = useState<string | null>(null);

  const statusLabel = useMemo(() => {
    if (isBusy) return "Working...";
    if (!status) return "Unknown";
    return formatPhase(status.phase);
  }, [isBusy, status]);

  const isRecording = status?.phase === "recording";

  useEffect(() => {
    void refreshStatus();
  }, []);

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
    setError(null);
    setExportMessage(null);

    try {
      const nextStatus = await invoke<RecordingStatus>("start_recording");
      setStatus(nextStatus);
    } catch (caught) {
      setError(formatError(caught));
      await refreshStatus(false);
    } finally {
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
      const exportStatus = await invoke<ExportStatus>("export_edited_mp4");
      setExportMessage("Export completed.");
      setExportOutputPath(exportStatus.outputPath);
    } catch (caught) {
      setError(formatError(caught));
    } finally {
      setIsBusy(false);
    }
  }

  return (
    <main style={styles.page}>
      <section style={styles.panel}>
        <h1 style={styles.title}>Screen Recorder</h1>

        <p style={styles.status}>
          Status: <strong>{statusLabel}</strong>
        </p>

        <div style={styles.controls}>
          <button
            type="button"
            onClick={startRecording}
            disabled={isBusy || isRecording}
            style={styles.button}
          >
            Start Recording
          </button>

          <button
            type="button"
            onClick={stopRecording}
            disabled={isBusy || !isRecording}
            style={styles.button}
          >
            Stop Recording
          </button>

          <button
            type="button"
            onClick={exportEditedVideo}
            disabled={isBusy || isRecording}
            style={styles.button}
          >
            Export Edited Video
          </button>
        </div>

        {status?.outputPath ? (
          <p style={styles.meta}>Output: {status.outputPath}</p>
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

const styles = {
  page: {
    minHeight: "100vh",
    display: "grid",
    placeItems: "center",
    background: "#f6f7f9",
    color: "#1f2937",
    fontFamily:
      "Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, Segoe UI, sans-serif",
  },
  panel: {
    width: "min(420px, calc(100vw - 32px))",
    padding: "24px",
    border: "1px solid #d7dce2",
    borderRadius: "8px",
    background: "#ffffff",
    boxShadow: "0 12px 32px rgba(15, 23, 42, 0.08)",
  },
  title: {
    margin: "0 0 16px",
    fontSize: "24px",
    fontWeight: 700,
  },
  status: {
    margin: "0 0 20px",
    fontSize: "16px",
  },
  controls: {
    display: "flex",
    gap: "12px",
    flexWrap: "wrap",
  },
  button: {
    minHeight: "40px",
    padding: "0 14px",
    border: "1px solid #aeb7c2",
    borderRadius: "6px",
    background: "#ffffff",
    color: "#111827",
    font: "inherit",
    cursor: "pointer",
  },
  meta: {
    margin: "16px 0 0",
    fontSize: "13px",
    lineHeight: 1.5,
    color: "#4b5563",
    overflowWrap: "anywhere",
  },
  error: {
    margin: "16px 0 0",
    fontSize: "14px",
    lineHeight: 1.5,
    color: "#b42318",
  },
  success: {
    margin: "16px 0 0",
    fontSize: "14px",
    lineHeight: 1.5,
    color: "#067647",
  },
} satisfies Record<string, CSSProperties>;

export default App;
