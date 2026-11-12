// Stable browser-only regression data. No server connection or production credential.
// Kept with the tests so documentation/design cleanups cannot remove their inputs.
(() => {
  const longSql = [
    'SELECT',
    '  u.id,',
    '  u.name,',
    '  p.total_amount',
    'FROM navkit_accept.t_profile AS p',
    'LEFT JOIN navkit_accept.t_user AS u',
    '  ON u.id = p.user_id',
    "WHERE u.name = '<script>&中文' AND p.note = '" + '完整原文与软换行'.repeat(80) + "'",
    'ORDER BY u.id',
    'LIMIT 100;',
  ].join('\n');
  const base = { ts: '2026-11-08 12:41:09', connection_id: 'c1', connection_name: '本地连接', kind: 'mysql', database: 'navkit_accept', ok: true, elapsed_ms: 1234, rows: 1, message: '', origin: 'query' };
  window.NAVKIT_DESIGN = {
    longSql,
    records: [
      { ...base, sql: longSql },
      { ...base, sql: 'SHOW CREATE TABLE navkit_accept.t_user;', elapsed_ms: 23 },
      { ...base, sql: ['UPDATE navkit_accept.t_user', "SET name = '中文姓名',", "    note = '保留完整 SQL',", '    active = 1', 'WHERE id = 7;'].join('\n'), rows: 1 },
      { ...base, sql: ['SELECT missing_column', 'FROM navkit_accept.t_user', 'WHERE id = 7;'].join('\n'), ok: false, rows: -1, message: "Unknown column 'missing_column' in 'field list'" },
    ],
    sessions: [
      { id: '940', user: 'fixture', host: '127.0.0.1', db: 'navkit_accept', active: false, self: true, elapsed: '0', sql: '' },
      { id: '941', user: 'fixture', host: '127.0.0.1', db: 'navkit_accept', active: true, self: false, elapsed: '1.25', sql: longSql },
      { id: '943', user: 'fixture', host: '127.0.0.1', db: 'navkit_accept', active: true, self: false, elapsed: '2', sql: 'SELECT COUNT(*) FROM navkit_accept.t_user;' },
      { id: '945', user: 'fixture', host: '127.0.0.1', db: 'navkit_accept', active: false, self: false, elapsed: '360', sql: '' },
      { id: '947', user: 'fixture', host: '127.0.0.1', db: 'navkit_accept', active: false, self: false, elapsed: '360', sql: '' },
    ],
  };
})();
