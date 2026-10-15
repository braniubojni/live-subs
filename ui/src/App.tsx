import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import ExpandMoreIcon from "@mui/icons-material/ExpandMore";
import InfoOutlinedIcon from "@mui/icons-material/InfoOutlined";
import {
  Accordion,
  AccordionDetails,
  AccordionSummary,
  Alert,
  Autocomplete,
  Box,
  Button,
  Checkbox,
  Chip,
  CircularProgress,
  Dialog,
  DialogActions,
  DialogContent,
  DialogContentText,
  DialogTitle,
  FormControlLabel,
  IconButton,
  LinearProgress,
  Stack,
  TextField,
  Tooltip,
  Typography,
} from "@mui/material";
import {
  useEffect,
  useRef,
  useState,
  type ReactNode,
  type SyntheticEvent,
} from "react";

type SlotKind = "stt" | "cleanup";

type Slot = {
  id: string;
  path: string;
  licence: string;
  present: boolean;
};

type Progress = { got: number; total: number };

type SettingsState = {
  stt: Slot | null;
  cleanup: Slot | null;
  language: string;
  prompt_carry: boolean;
  device: string | null;
  running: boolean;
  error: string | null;
  stt_progress: Progress | null;
  cleanup_progress: Progress | null;
};

type Snapshot = { state: SettingsState };

type ModelChoice = {
  id: string;
  name: string;
  licence: string;
  path: string;
  present: boolean;
};

type LangRow = { code: string; name: string };
type DeviceRow = { name: string | null; label: string };
type HfHit = {
  repo: string;
  filename: string;
  size: number;
  sha256: string;
  licence: string;
  url: string;
};

type Confirm =
  | { kind: "one"; slot: SlotKind; name: string }
  | { kind: "all" };

function useSnapshot(): Snapshot | null {
  const [snap, setSnap] = useState<Snapshot | null>(null);
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let dead = false;
    void listen<Snapshot>("snapshot", (event) => {
      setSnap(event.payload);
    }).then((stop) => {
      if (dead) stop();
      else unlisten = stop;
    });
    void invoke<Snapshot>("snapshot").then(setSnap);
    return () => {
      dead = true;
      unlisten?.();
    };
  }, []);
  return snap;
}

function Hint({ title }: { title: string }) {
  return (
    <Tooltip title={title}>
      <IconButton size="small" aria-label={title}>
        <InfoOutlinedIcon fontSize="small" />
      </IconButton>
    </Tooltip>
  );
}

function WithHint({ hint, children }: { hint: string; children: ReactNode }) {
  return (
    <Stack direction="row" spacing={0.5} sx={{ alignItems: "center" }}>
      <Box sx={{ flex: 1, minWidth: 0 }}>{children}</Box>
      <Hint title={hint} />
    </Stack>
  );
}

function summary(title: string) {
  return <AccordionSummary expandIcon={<ExpandMoreIcon />}>{title}</AccordionSummary>;
}

function ModelPicker({
  kind,
  selected,
}: {
  kind: SlotKind;
  selected: Slot | null;
}) {
  const [options, setOptions] = useState<ModelChoice[]>([]);
  const [loading, setLoading] = useState(false);
  const req = useRef(0);

  async function load(query: string) {
    const id = ++req.current;
    setLoading(true);
    const rows = await invoke<ModelChoice[]>("models", { kind, query });
    if (req.current === id) {
      setOptions(rows);
      setLoading(false);
    }
  }

  const value: ModelChoice | null = selected
    ? {
        id: selected.id,
        name: options.find((row) => row.id === selected.id)?.name ?? selected.id,
        licence: selected.licence,
        path: selected.path,
        present: selected.present,
      }
    : null;

  return (
    <Autocomplete
      options={
        value && !options.some((row) => row.id === value.id)
          ? [value, ...options]
          : options
      }
      loading={loading}
      value={value}
      filterOptions={(rows) => rows}
      isOptionEqualToValue={(a, b) => a.id === b.id}
      getOptionLabel={(row) =>
        row.present ? `${row.name} — downloaded` : row.name
      }
      onOpen={() => {
        void load("");
      }}
      onInputChange={(_, query, reason) => {
        if (reason === "input" || reason === "clear") void load(query);
      }}
      onChange={(_, row) => {
        if (!row) return;
        void invoke("event", {
          event: {
            type: "select",
            kind,
            id: row.id,
            path: row.path,
            licence: row.licence,
            present: row.present,
          },
        });
      }}
      renderInput={(params) => <TextField {...params} label="Model" />}
    />
  );
}

function LanguagePicker({ language }: { language: string }) {
  const [options, setOptions] = useState<LangRow[]>([]);
  const [loading, setLoading] = useState(false);
  const req = useRef(0);

  async function load(query: string) {
    const id = ++req.current;
    setLoading(true);
    const rows = await invoke<LangRow[]>("languages", { query });
    if (req.current === id) {
      setOptions(rows);
      setLoading(false);
    }
  }

  const known = options.find((row) => row.code === language);
  const value = { code: language, name: known?.name ?? language };

  return (
    <Autocomplete
      options={
        options.some((row) => row.code === value.code) ? options : [value, ...options]
      }
      loading={loading}
      value={value}
      filterOptions={(rows) => rows}
      isOptionEqualToValue={(a, b) => a.code === b.code}
      getOptionLabel={(row) => `${row.name} (${row.code})`}
      onOpen={() => {
        void load("");
      }}
      onInputChange={(_, query, reason) => {
        if (reason === "input" || reason === "clear") void load(query);
      }}
      onChange={(_, row) => {
        if (!row) return;
        void invoke("event", { event: { type: "language", code: row.code } });
      }}
      renderInput={(params) => (
        <TextField {...params} label="Spoken language" />
      )}
    />
  );
}

function DevicePicker({ device }: { device: string | null }) {
  const [options, setOptions] = useState<DeviceRow[]>([]);
  const [loading, setLoading] = useState(false);
  const req = useRef(0);

  async function load(query: string) {
    const id = ++req.current;
    setLoading(true);
    const rows = await invoke<DeviceRow[]>("devices", { query });
    if (req.current === id) {
      setOptions(rows);
      setLoading(false);
    }
  }

  const value: DeviceRow = {
    name: device,
    label: device ?? "Default",
  };

  return (
    <Autocomplete
      options={
        options.some((row) => row.name === value.name) ? options : [value, ...options]
      }
      loading={loading}
      value={value}
      filterOptions={(rows) => rows}
      isOptionEqualToValue={(a, b) => a.name === b.name}
      getOptionLabel={(row) => row.label}
      onOpen={() => {
        void load("");
      }}
      onInputChange={(_, query, reason) => {
        if (reason === "input" || reason === "clear") void load(query);
      }}
      onChange={(_, row) => {
        if (!row) return;
        void invoke("event", { event: { type: "device", name: row.name } });
      }}
      renderInput={(params) => <TextField {...params} label="Device" />}
    />
  );
}

function formatSize(size: number): string {
  if (size === 0) return "0 bytes";
  const units = ["bytes", "KB", "MB", "GB"];
  let n = size;
  let i = 0;
  while (n >= 1024 && i < units.length - 1) {
    n /= 1024;
    i += 1;
  }
  return `${n.toFixed(i === 0 ? 0 : 1)} ${units[i]}`;
}

function HfPicker() {
  const [options, setOptions] = useState<HfHit[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [input, setInput] = useState("");
  const [value, setValue] = useState<HfHit | null>(null);
  const req = useRef(0);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);

  function fetchQuery(query: string) {
    const id = ++req.current;
    setLoading(true);
    setError(null);
    void invoke<HfHit[]>("search_hf", { query })
      .then((rows) => {
        if (req.current === id) setOptions(rows);
      })
      .catch((err: unknown) => {
        if (req.current !== id) return;
        setOptions([]);
        setError(err instanceof Error ? err.message : String(err));
      })
      .finally(() => {
        if (req.current === id) setLoading(false);
      });
  }

  function schedule(query: string, immediate: boolean) {
    clearTimeout(timer.current);
    const q = query.trim() || "whisper";
    if (immediate) fetchQuery(q);
    else timer.current = setTimeout(() => fetchQuery(q), 400);
  }

  return (
    <Autocomplete
      openOnFocus
      options={
        value && !options.some((row) => row.url === value.url)
          ? [value, ...options]
          : options
      }
      loading={loading}
      value={value}
      filterOptions={(rows) => rows}
      isOptionEqualToValue={(a, b) => a.url === b.url}
      getOptionLabel={(row) =>
        `${row.repo} / ${row.filename} · ${formatSize(row.size)} · ${row.licence}`
      }
      onOpen={() => schedule(input, true)}
      onInputChange={(_, query, reason) => {
        if (reason === "input") {
          setInput(query);
          schedule(query, false);
        } else if (reason === "clear") {
          setInput("");
          schedule("", true);
        }
      }}
      onChange={(_, row) => {
        setValue(row);
        if (!row) return;
        void invoke("pick_hf", { hit: row });
      }}
      noOptionsText={error ?? "No models"}
      renderInput={(params) => (
        <TextField
          {...params}
          label="Hugging Face"
          placeholder="whisper, qwen, …"
          InputProps={{
            ...params.InputProps,
            endAdornment: (
              <>
                {loading ? <CircularProgress color="inherit" size={18} /> : null}
                {params.InputProps.endAdornment}
              </>
            ),
          }}
        />
      )}
    />
  );
}

function ProgressBar({ progress }: { progress: Progress | null }) {
  if (!progress) return null;
  const determinate = progress.total > 0;
  return (
    <LinearProgress
      variant={determinate ? "determinate" : "indeterminate"}
      value={determinate ? (progress.got / progress.total) * 100 : undefined}
    />
  );
}

type Panel = "speech" | "cleanup" | "hf" | "device" | "run";

function ModelPanel({
  title,
  kind,
  slot,
  progress,
  expanded,
  onChange,
  onRemove,
}: {
  title: string;
  kind: SlotKind;
  slot: Slot | null;
  progress: Progress | null;
  expanded: boolean;
  onChange: (event: SyntheticEvent, isExpanded: boolean) => void;
  onRemove: () => void;
}) {
  return (
    <Accordion expanded={expanded} onChange={onChange} {...groupAccordion}>
      {summary(title)}
      <AccordionDetails>
        <Stack spacing={1.5}>
          <WithHint
            hint={
              kind === "stt"
                ? "Turns the captured audio into text."
                : "Rewrites that text into shorter subtitle lines. You can leave this empty."
            }
          >
            <ModelPicker kind={kind} selected={slot} />
          </WithHint>
          {slot ? (
            <Stack direction="row" spacing={1} sx={{ alignItems: "center" }}>
              <Typography variant="body2">Licence: {slot.licence}</Typography>
              {slot.present ? <Chip size="small" label="downloaded" /> : null}
            </Stack>
          ) : null}
          <Stack direction="row" spacing={1}>
            <Button onClick={() => void invoke("browse", { kind })}>Browse</Button>
            <Button
              disabled={progress !== null}
              onClick={() => void invoke("download", { kind })}
            >
              Download
            </Button>
            <Button
              disabled={progress === null}
              onClick={() => void invoke("cancel_download", { kind })}
            >
              Cancel
            </Button>
            <Button disabled={!slot?.present} onClick={onRemove}>
              Remove
            </Button>
          </Stack>
          <ProgressBar progress={progress} />
        </Stack>
      </AccordionDetails>
    </Accordion>
  );
}

const groupAccordion = {
  disableGutters: true,
  elevation: 0,
  square: true,
  sx: {
    border: 1,
    borderColor: "divider",
    "&:not(:last-child)": { borderBottom: 0 },
    "&::before": { display: "none" },
  },
} as const;

export function App() {
  const snap = useSnapshot();
  const [confirm, setConfirm] = useState<Confirm | null>(null);
  const [open, setOpen] = useState<Panel | false>("speech");
  const state = snap?.state;

  function toggle(panel: Panel) {
    return (_event: SyntheticEvent, isExpanded: boolean) => {
      setOpen(isExpanded ? panel : false);
    };
  }

  return (
    <Box sx={{ p: 2 }}>
      <Typography variant="h6" sx={{ mb: 1 }}>
        live-subs
      </Typography>
      {state ? (
        <Stack>
          <ModelPanel
            title="Speech"
            kind="stt"
            slot={state.stt}
            progress={state.stt_progress}
            expanded={open === "speech"}
            onChange={toggle("speech")}
            onRemove={() =>
              setConfirm({
                kind: "one",
                slot: "stt",
                name: state.stt?.id ?? "speech model",
              })
            }
          />
          <ModelPanel
            title="Cleanup"
            kind="cleanup"
            slot={state.cleanup}
            progress={state.cleanup_progress}
            expanded={open === "cleanup"}
            onChange={toggle("cleanup")}
            onRemove={() =>
              setConfirm({
                kind: "one",
                slot: "cleanup",
                name: state.cleanup?.id ?? "cleanup model",
              })
            }
          />
          <Accordion {...groupAccordion} expanded={open === "hf"} onChange={toggle("hf")}>
            {summary("Hugging Face")}
            <AccordionDetails>
              <WithHint hint="Search Hugging Face for a model file. A .bin file fills the speech panel; a .gguf file fills cleanup. Pick a row, then Download on that panel.">
                <HfPicker />
              </WithHint>
            </AccordionDetails>
          </Accordion>
          <Accordion
            {...groupAccordion}
            expanded={open === "device"}
            onChange={toggle("device")}
          >
            {summary("Device and language")}
            <AccordionDetails>
              <Stack spacing={1.5}>
                <WithHint hint="Audio device to listen to. Default follows the current speakers or headphones.">
                  <DevicePicker device={state.device} />
                </WithHint>
                <WithHint hint="Language being spoken in the audio. Whisper writes that language down; it does not translate. Auto detects it.">
                  <LanguagePicker language={state.language} />
                </WithHint>
                <WithHint hint="Passes the last subtitle into the next recognition, so names and phrasing stay consistent.">
                  <FormControlLabel
                    control={
                      <Checkbox
                        checked={state.prompt_carry}
                        onChange={(event) =>
                          void invoke("event", {
                            event: { type: "prompt_carry", on: event.target.checked },
                          })
                        }
                      />
                    }
                    label="Use the previous line as a hint"
                  />
                </WithHint>
              </Stack>
            </AccordionDetails>
          </Accordion>
          <Accordion {...groupAccordion} expanded={open === "run"} onChange={toggle("run")}>
            {summary("Run")}
            <AccordionDetails>
              <Stack spacing={1.5}>
                {state.error ? <Alert severity="error">{state.error}</Alert> : null}
                <Stack direction="row" spacing={1}>
                  <Button
                    variant="contained"
                    disabled={state.running}
                    onClick={() => void invoke("start")}
                  >
                    {state.running ? "Running" : "Start"}
                  </Button>
                  <Button
                    disabled={!state.running}
                    onClick={() => void invoke("stop")}
                  >
                    Stop
                  </Button>
                  <Button onClick={() => setConfirm({ kind: "all" })}>
                    Delete all downloaded
                  </Button>
                </Stack>
              </Stack>
            </AccordionDetails>
          </Accordion>
        </Stack>
      ) : (
        <LinearProgress />
      )}
      <Dialog open={confirm !== null} onClose={() => setConfirm(null)}>
        <DialogTitle>Delete models</DialogTitle>
        <DialogContent>
          <DialogContentText>
            {confirm?.kind === "one"
              ? `Delete ${confirm.name}?`
              : "Every downloaded file in the models folder will be deleted."}
          </DialogContentText>
        </DialogContent>
        <DialogActions>
          <Button onClick={() => setConfirm(null)}>Cancel</Button>
          <Button
            color="error"
            onClick={() => {
              if (confirm?.kind === "one") {
                void invoke("remove_one", { kind: confirm.slot });
              } else if (confirm?.kind === "all") {
                void invoke("remove_all");
              }
              setConfirm(null);
            }}
          >
            Delete
          </Button>
        </DialogActions>
      </Dialog>
    </Box>
  );
}
