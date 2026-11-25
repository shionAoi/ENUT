import type { Agent, Snapshot } from './types.ts';
import { tr, type Lang, type TranslationKey } from './i18n.ts';
import { presentAgent } from './presentation.ts';

type Command = 'scan_agents' | 'refresh_usage' | 'set_permission' | 'connect_billing' | 'disconnect_billing';
export type Connection = (command: Command, args?: Record<string, unknown>) => Promise<Snapshot>;

export class Monitor {
  snapshot: Snapshot = { agents: [], scannedAt: '', platform: '', warnings: [] };
  busy = false;
  failures = 0;
  private noticeKey: TranslationKey | null = null;
  private customNotice = '';
  readonly approved = new Set<string>();
  private epoch = 0;
  private pendingPermissions = 0;
  private permissionVersions = new Map<string, number>();
  private billingVersions = new Map<string, number>();
  private refreshing = false;
  private permissionOperations = new Map<Agent['id'], Promise<void>>();
  private billingOperation: Promise<void> = Promise.resolve();
  private revoked = new Set<string>();
  private disconnectedBilling = new Set<string>();

  readonly desktop: boolean;
  private invoke: Connection;
  private changed: () => void;
  private getLang: () => Lang;
  constructor(desktop: boolean, invoke: Connection, changed: () => void, getLang: () => Lang = () => 'en') {
    this.desktop = desktop;
    this.invoke = invoke;
    this.changed = changed;
    this.getLang = getLang;
    if (!desktop) this.noticeKey = 'desktopRequiredTitle';
  }
  get notice(): string {
    return this.customNotice || (this.noticeKey ? tr(this.getLang(), this.noticeKey) : '');
  }
  set notice(value: string) {
    this.customNotice = value;
    this.noticeKey = null;
  }
  setNoticeKey(key: TranslationKey | null): void {
    this.noticeKey = key;
    this.customNotice = '';
  }
  private clear(agent: Agent): Agent {
    return { ...agent, connected: false, monitoringAllowed: false, historyCached: false, running: null, usage: null, reportedPeriodUsage: null, savedPeriodUsage: null, quotas: [], models: [], usageHistory: [], modelUsage: [], partialUsage: null, modelHistory: [], context: null, authentication:null, activeSessions:null, updatedAt: null,
      message: tr(this.getLang(), 'accessNotGranted'), status: 'detected' };
  }
  private visibleBilling(snapshot: Snapshot) {
    return snapshot.billing?.filter(report => !this.disconnectedBilling.has(report.provider));
  }
  private publish(snapshot: Snapshot, tool?: Agent['id']) {
    const incoming = tool === undefined ? snapshot.agents : snapshot.agents.filter(a => a.id === tool);
    for (const agent of incoming) {
      if (!this.revoked.has(agent.id)) {
        if (agent.monitoringAllowed === true || (agent.monitoringAllowed === undefined && agent.connected)) this.approved.add(agent.id);
        else if (agent.monitoringAllowed === false) this.approved.delete(agent.id);
      }
    }
    const projected = incoming.filter(a => a.installed).map(a => presentAgent(this.revoked.has(a.id) ? this.clear(a) : a));
    if (tool === undefined) {
      this.snapshot = { ...snapshot, billing: this.visibleBilling(snapshot), agents: projected };
    } else {
      const agents = [...this.snapshot.agents];
      const index = agents.findIndex(a => a.id === tool);
      if (index === -1) agents.push(...projected);
      else agents.splice(index, 1, ...projected);
      this.snapshot = { ...this.snapshot, agents };
    }
  }
  async pause(agentId: Agent['id']): Promise<void> {
    const epoch = ++this.epoch;
    this.permissionVersions.set(agentId, epoch);
    this.approved.delete(agentId);
    this.revoked.add(agentId);
    this.publish(this.snapshot, agentId);
    this.changed();
    // auth_start owns the backend pause; never route login through destructive Revoke.
    await this.permissionOperations.get(agentId);
  }
  async refresh(scan = false): Promise<void> {
    if (!this.desktop || this.busy) return;
    this.busy = true;
    this.refreshing = true;
    const epoch = this.epoch;
    this.changed();
    try {
      const result = await this.invoke(scan ? 'scan_agents' : 'refresh_usage');
      if (epoch === this.epoch) {
        if (scan) {
          for (const id of this.approved) {
            if (!result.agents.some(a => a.id === id && a.installed)) this.approved.delete(id);
          }
        }
        this.publish(result);
        this.setNoticeKey(null);
      }
      this.failures = 0;
    } catch {
      if (epoch !== this.epoch) return;
      this.failures++;
      this.snapshot={...this.snapshot,agents:this.snapshot.agents.map(agent=>({...agent,activeSessions:null,authentication:null,reportedPeriodUsage:null}))};
      this.setNoticeKey('noticeUpdateFailed');
    } finally { this.refreshing = false; this.busy = this.pendingPermissions > 0; this.changed(); }
  }
  async billing(provider: 'openai' | 'anthropic', credential?: string): Promise<void> {
    if (!this.desktop) return;
    const epoch = ++this.epoch;
    this.billingVersions.set(provider, epoch);
    this.pendingPermissions++;
    this.busy = true;
    if (credential === undefined) {
      this.disconnectedBilling.add(provider);
      this.snapshot = { ...this.snapshot, billing: this.snapshot.billing?.filter(r => r.provider !== provider) };
    }
    this.changed();
    let connected = false;
    const previous = this.billingOperation;
    const operation = (async () => {
      await previous;
      try {
        if (this.billingVersions.get(provider) !== epoch) return;
        const isolated = !this.refreshing && this.pendingPermissions === 1;
        const result = await this.invoke(credential === undefined ? 'disconnect_billing' : 'connect_billing', credential === undefined ? { provider } : { provider, credential });
        if (this.billingVersions.get(provider) !== epoch) return;
        if (credential !== undefined) {
          this.disconnectedBilling.delete(provider);
          connected = true;
        }
        if (isolated && epoch === this.epoch) {
          this.publish(result);
          this.setNoticeKey(null);
        } else {
          const current = this.snapshot.billing?.filter(report => report.provider !== provider) ?? [];
          const updated = this.visibleBilling(result)?.filter(report => report.provider === provider) ?? [];
          this.snapshot = { ...this.snapshot, billing: [...current, ...updated].sort((a, b) => a.provider.localeCompare(b.provider)) };
        }
      } catch {
        this.setNoticeKey(credential === undefined ? 'noticeReportingDisconnectFailed' : 'noticeReportingConnectFailed');
      } finally { this.pendingPermissions--; this.busy = this.refreshing || this.pendingPermissions > 0; this.changed(); }
    })();
    this.billingOperation = operation;
    await operation;
    if (connected && this.billingVersions.get(provider) === epoch) await this.refresh();
  }
  async permission(agentId: Agent['id'], allowed: boolean): Promise<void> {
    if (!this.desktop) return;
    const epoch = ++this.epoch;
    this.permissionVersions.set(agentId, epoch);
    this.pendingPermissions++;
    this.busy = true;
    if (!allowed) {
      this.approved.delete(agentId);
      this.revoked.add(agentId);
      this.publish(this.snapshot, agentId);
      this.changed();
    }
    const previous = this.permissionOperations.get(agentId);
    const operation = (async () => {
      await previous;
      this.busy = true;
      this.changed();
      try {
        if (this.permissionVersions.get(agentId) !== epoch) return;
        const isolated = !this.refreshing && this.pendingPermissions === 1;
        const result = await this.invoke('set_permission', { agentId, allowed });
        if (this.permissionVersions.get(agentId) === epoch) {
          if (allowed) { this.revoked.delete(agentId); this.approved.add(agentId); }
          // An overlapping reply may contain stale data from another tool or billing lane.
          if (isolated && epoch === this.epoch) {
            this.publish(result);
            this.setNoticeKey(null);
          } else {
            this.publish(result, agentId);
          }
        }
      } catch {
        if (this.permissionVersions.get(agentId) === epoch) {
          this.setNoticeKey(allowed ? 'noticePermissionGrantFailed' : 'noticeRevokeFailed');
        }
      } finally { this.pendingPermissions--; this.busy = this.refreshing || this.pendingPermissions > 0; this.changed(); }
    })();
    this.permissionOperations.set(agentId, operation);
    await operation;
    if (this.permissionOperations.get(agentId) === operation) this.permissionOperations.delete(agentId);
  }
}
