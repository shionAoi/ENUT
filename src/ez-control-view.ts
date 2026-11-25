import { tr, type Lang } from './i18n.ts';
import { presentAgent } from './presentation.ts';
import type { ProcessControlSnapshot } from './types.ts';
import { escapeHtml as e } from './view.ts';

function clamp(n: number, min: number, max: number): number {
  return Number.isFinite(n) ? Math.min(max, Math.max(min, n)) : min;
}

export function formatProcessMemory(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return '0MB';
  if (bytes < 1048576) return `${Math.max(1, Math.round(bytes / 1024))}KB`;
  if (bytes < 1073741824) return `${Math.round(bytes / 1048576)}MB`;
  return `${(bytes / 1073741824).toFixed(1)}GB`;
}

export function formatCpuPercent(cpu: number): string {
  if (!Number.isFinite(cpu) || cpu < 0.05) return '0%';
  if (cpu < 1) return `${cpu.toFixed(1)}%`;
  return `${Math.round(clamp(cpu, 0, 100))}%`;
}

function polar(cx: number, cy: number, r: number, angleDeg: number): [number, number] {
  const rad = (angleDeg * Math.PI) / 180;
  return [cx + r * Math.cos(rad), cy + r * Math.sin(rad)];
}

function arcPath(cx: number, cy: number, r: number, startDeg: number, sweepDeg: number): string {
  if (sweepDeg <= 0.01) return '';
  const safeSweep = Math.min(sweepDeg, 269.9);
  const [x1, y1] = polar(cx, cy, r, startDeg);
  const [x2, y2] = polar(cx, cy, r, startDeg + safeSweep);
  const large = safeSweep > 180 ? 1 : 0;
  return `M ${x1.toFixed(2)} ${y1.toFixed(2)} A ${r} ${r} 0 ${large} 1 ${x2.toFixed(2)} ${y2.toFixed(2)}`;
}

interface GaugeOptions {
  title: string;
  valueText: string;
  unit: string;
  minLabel: string;
  midLabel: string;
  maxLabel: string;
  ratio: number;
  now: number;
  max: number;
  ariaText: string;
}

function renderGauge(opt: GaugeOptions): string {
  const cx = 75;
  const cy = 78;
  const startDeg = 135;
  const fullSweep = 270;
  const ratio = clamp(opt.ratio, 0, 1);
  const activeSweep = fullSweep * ratio;
  const trackOuter = arcPath(cx, cy, 56, startDeg, fullSweep);
  const trackArc = arcPath(cx, cy, 48, startDeg, fullSweep);
  const valueArc = activeSweep > 0.5 ? arcPath(cx, cy, 48, startDeg, activeSweep) : '';
  const ticks = [0, 0.125, 0.25, 0.375, 0.5, 0.625, 0.75, 0.875, 1]
    .map((f, idx) => {
      const deg = startDeg + fullSweep * f;
      const major = idx % 2 === 0;
      const [x1, y1] = polar(cx, cy, 52, deg);
      const [x2, y2] = polar(cx, cy, major ? 41 : 45, deg);
      return `<line x1="${x1.toFixed(2)}" y1="${y1.toFixed(2)}" x2="${x2.toFixed(2)}" y2="${y2.toFixed(2)}" class="${major ? 'ez-tick-major' : 'ez-tick-minor'}"/>`;
    })
    .join('');
  const [nx1, ny1] = polar(cx, cy, 31, startDeg + activeSweep);
  const [nx2, ny2] = polar(cx, cy, 50, startDeg + activeSweep);
  const needle = ratio > 0.002
    ? `<line x1="${nx1.toFixed(2)}" y1="${ny1.toFixed(2)}" x2="${nx2.toFixed(2)}" y2="${ny2.toFixed(2)}" class="ez-needle"/>`
    : '';

  return `<section class="ez-gauge" aria-label="${e(opt.title)}">
    <h3 class="ez-gauge-title">${e(opt.title)}</h3>
    <div class="ez-gauge-wrap" role="meter" aria-label="${e(opt.title)}" aria-valuemin="0" aria-valuemax="${opt.max}" aria-valuenow="${opt.now}" aria-valuetext="${e(opt.ariaText)}">
      <svg class="ez-gauge-svg" viewBox="0 0 150 144" aria-hidden="true">
        <path d="${trackOuter}" class="ez-ring-outer"/>
        <path d="${trackArc}" class="ez-ring-track"/>
        ${valueArc ? `<path d="${valueArc}" class="ez-ring-fill"/>` : ''}
        <circle cx="${cx}" cy="${cy}" r="33" class="ez-dial-inner"/>
        ${ticks}
        ${needle}
        <text x="69" y="14" text-anchor="end" class="ez-scale-label">${e(opt.midLabel)}</text>
        <polygon points="75,6 79,10 75,14 71,10" class="ez-mid-diamond"/>
        <text x="27" y="131" text-anchor="middle" class="ez-scale-label">${e(opt.minLabel)}</text>
        <text x="123" y="131" text-anchor="middle" class="ez-scale-label">${e(opt.maxLabel)}</text>
      </svg>
      <div class="ez-gauge-readout"><strong>${e(opt.valueText)}</strong>${opt.unit ? `<span>${e(opt.unit)}</span>` : ''}</div>
    </div>
  </section>`;
}

export function renderEzControl(
  snapshot: ProcessControlSnapshot | null,
  approved: Set<string>,
  collapsed: Set<string>,
  desktop: boolean,
  lang: Lang = 'en',
): string {
  const groups = (snapshot?.agents ?? [])
    .filter(group => approved.has(group.agentId))
    .map(group => ({
      ...group,
      displayName: presentAgent({ id: group.agentId, name: group.name } as Parameters<typeof presentAgent>[0]).name,
    }));

  const usedMemoryBytes = groups.reduce((sum, g) => sum + (Number.isFinite(g.memoryBytes) ? Math.max(0, g.memoryBytes) : 0), 0);
  const usedCpuPercent = clamp(
    groups.reduce((sum, g) => sum + (Number.isFinite(g.cpuPercent) ? Math.max(0, g.cpuPercent) : 0), 0),
    0,
    100,
  );

  const systemBytes = snapshot && Number.isFinite(snapshot.systemMemoryBytes) && snapshot.systemMemoryBytes > 0
    ? snapshot.systemMemoryBytes
    : 16 * 1073741824;
  const maxGb = Math.max(1, Math.round(systemBytes / 1073741824));
  const midGb = Math.round(maxGb / 2);
  const usedGbNum = usedMemoryBytes / 1073741824;
  const usedGbText = usedGbNum.toFixed(1);
  const cpuText = usedCpuPercent >= 0.05 && usedCpuPercent < 1
    ? `${usedCpuPercent.toFixed(1)}%`
    : `${Math.round(usedCpuPercent)}%`;

  const gauges = `<div class="ez-gauges">
    ${renderGauge({
      title: tr(lang, 'ramUsageHeading'),
      valueText: usedGbText,
      unit: 'GB',
      minLabel: '0',
      midLabel: String(midGb),
      maxLabel: String(maxGb),
      ratio: usedMemoryBytes / systemBytes,
      now: Number(usedGbNum.toFixed(2)),
      max: maxGb,
      ariaText: `${usedGbText} GB`,
    })}
    ${renderGauge({
      title: tr(lang, 'cpuUsageHeading'),
      valueText: cpuText,
      unit: '',
      minLabel: '0',
      midLabel: '50',
      maxLabel: '100',
      ratio: usedCpuPercent / 100,
      now: Number(usedCpuPercent.toFixed(1)),
      max: 100,
      ariaText: cpuText,
    })}
  </div>`;

  const body = groups.length
    ? groups.map(group => {
        const open = !collapsed.has(group.agentId);
        const parentRow = `<tr class="ez-agent-row">
          <th scope="row"><button type="button" class="ez-agent-toggle" data-control-agent="${e(group.agentId)}" aria-expanded="${open}"><span>${e(group.displayName)}</span><span class="ez-caret" aria-hidden="true">${open ? '⌄' : '›'}</span></button></th>
          <td>${e(formatProcessMemory(group.memoryBytes))}</td>
          <td>${e(formatCpuPercent(group.cpuPercent))}</td>
          <td></td>
        </tr>`;
        const childRows = open
          ? group.processes.map(proc => `<tr class="ez-task-row">
              <td class="ez-task-name">${e(proc.task)}</td>
              <td>${e(formatProcessMemory(proc.memoryBytes))}</td>
              <td>${e(formatCpuPercent(proc.cpuPercent))}</td>
              <td>${proc.pid}</td>
            </tr>`).join('')
          : '';
        return parentRow + childRows;
      }).join('')
    : `<tr class="ez-empty-row"><td colspan="4">${e(
        !desktop
          ? tr(lang, 'controlBrowserNote')
          : approved.size === 0
            ? tr(lang, 'controlConnectNote')
            : tr(lang, 'controlIdleNote'),
      )}</td></tr>`;

  return `${gauges}<div class="table-scroll"><table class="ez-control-table"><thead><tr><th scope="col">${e(tr(lang, 'colTask'))}</th><th scope="col">${e(tr(lang, 'colMemory'))}</th><th scope="col">${e(tr(lang, 'colCpu'))}</th><th scope="col">${e(tr(lang, 'colPid'))}</th></tr></thead><tbody>${body}</tbody></table></div>`;
}
