import type { ConnectionRequest } from "./types";

/** Secrets only travel directly to the local profile service; they never belong in ConnectionConfig.params. */
export type RuntimeSecretPatch = { action: "keep" } | { action: "clear" } | { action: "set"; value: string };
export type RuntimeTlsMode = "disable" | "prefer" | "require" | "verify_ca" | "verify_full";
export interface RuntimeTlsProfile { mode: RuntimeTlsMode; ca: string | null; cert: string | null; key: string | null }
export interface RuntimeSshProfile { host: string; port: number; user: string; authentication: "password" | "private_key"; key_file: string | null; known_hosts: string }
/** Outer HTTPS only. Database credentials/TLS are preconfigured on the bridge and never uploaded here. */
export interface RuntimeHttpProfile {
  endpoint: string;
  protocol: "navkit-sql-tunnel/1";
  expected_server_id: string;
  expected_target_id: string;
  target_alias: string;
  tls: RuntimeTlsProfile;
  connect_timeout_ms: number;
}
export interface RuntimeProfileDraft {
  ssh: RuntimeSshProfile | null;
  tls: RuntimeTlsProfile | null;
  ssh_password: RuntimeSecretPatch;
  ssh_key_passphrase: RuntimeSecretPatch;
  /** Optional for older callers; the backend defaults absent HTTP fields to null/Keep. */
  http?: RuntimeHttpProfile | null;
  http_bearer?: RuntimeSecretPatch;
}
export interface RuntimeProfileView {
  connection_id: string;
  endpoint_fingerprint: string;
  revision: string;
  ssh: RuntimeSshProfile | null;
  tls: RuntimeTlsProfile | null;
  ssh_password_present: boolean;
  ssh_key_passphrase_present: boolean;
  /** Older profile receipts may omit both HTTP fields; HTTP needs explicit token presence. */
  http?: RuntimeHttpProfile | null;
  http_bearer_present?: boolean;
}
export interface RuntimeProfileSubmission { draft: RuntimeProfileDraft; expected_revision: string | null; rebind_endpoint?: boolean }
export interface RuntimeProfileBindings {
  /** Backend computes principal, owner and endpoint fingerprint from the real window + saved config. */
  load(connectionId: string): Promise<RuntimeProfileView | null>;
  /** A transient test must not save credentials or create a persistent profile. */
  test(request: ConnectionRequest, connectionId: string, profile: RuntimeProfileSubmission, preserveDatabasePassword: boolean): Promise<{ ok: boolean; version?: string | null; message?: string | null }>;
}
