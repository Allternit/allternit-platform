import { describe, expect, it } from 'vitest';
import { pickComputers, planReenable } from './files-drive-logic.js';

describe('pickComputers', () => {
  it('lists other computers, not phones or this Mac', () => {
    const { computers, meshOnly } = pickComputers(
      [
        { id: 'rt_self', name: 'This Mac', runtimeType: 'desktop' },
        { id: 'rt_cloud', name: 'Cloud computer', runtimeType: 'cloud' },
        { id: 'rt_phone', name: 'iPhone', runtimeType: 'ios' },
        { id: 'rt_moto', name: 'Moto', runtimeType: 'android' },
        { id: 'rt_box', name: '', hostname: 'studio-box', runtimeType: 'desktop' },
      ],
      [],
      'rt_self',
    );
    expect(computers.map((c) => c.id)).toEqual(['rt_cloud', 'rt_box']);
    expect(computers[1].name).toBe('studio-box');
    expect(meshOnly).toEqual([]);
  });

  it('adds own mesh-only machines once, never shared, cloud or unjoined ones', () => {
    const { computers, meshOnly } = pickComputers(
      [{ id: 'rt_lab', name: 'Lab PC', runtimeType: 'desktop' }],
      [
        { id: 'pc_lab', name: 'lab pc', mesh_ip: '100.64.0.9' },
        { id: 'pc_nas', name: 'NAS', mesh_ip: '100.64.0.7' },
        { id: 'pc_cloud', name: 'Cloud', mesh_ip: '100.64.0.8', cloud_computer: true },
        { id: 'pc_team', name: 'Team box', mesh_ip: '100.64.0.6', shared: true },
        { id: 'pc_new', name: 'Not joined yet', mesh_ip: null },
      ],
      undefined,
    );
    expect(computers.map((c) => c.id)).toEqual(['rt_lab', 'pc_nas']);
    expect(meshOnly).toEqual([{ id: 'pc_nas', meshIp: '100.64.0.7' }]);
  });
});

describe('planReenable', () => {
  const on = (computerId: string) => ({ computerId, name: computerId, userEnabled: true });
  const off = (computerId: string) => ({ computerId, name: computerId, userEnabled: false });

  it('re-adds a location once after the user turns the provider on', () => {
    let state = planReenable([off('a')], {}, ['a']);
    expect(state.reenable).toEqual([]);
    state = planReenable([on('a')], state.seen);
    expect(state.reenable).toEqual(['a']);
    state = planReenable([on('a')], state.seen);
    expect(state.reenable).toEqual([]);
  });

  it('leaves a location added while the provider was already on', () => {
    expect(planReenable([on('a')], {}, ['a']).reenable).toEqual([]);
  });

  it('re-adds again after the user turns it off and on', () => {
    let state = planReenable([on('a')], { a: true });
    state = planReenable([off('a')], state.seen);
    state = planReenable([on('a')], state.seen);
    expect(state.reenable).toEqual(['a']);
  });

  it('forgets computers whose location is gone', () => {
    expect(planReenable([on('b')], { a: true, b: true }).seen).toEqual({ b: true });
  });
});
