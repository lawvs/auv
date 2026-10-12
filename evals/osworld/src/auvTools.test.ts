import { describe, expect, it } from 'vitest'

import { invokeCatalog, toolDocsFrom } from './auvTools'

const schema = {
  'properties': { command_id: { enum: ['input.pressKeys', 'display.capture'], type: 'string' } },
  'x-auv-commands': [
    {
      arguments: [
        { help: 'One key combination. MCP inputs encode this list as a JSON array string', input_key: 'keys', repeated: true, required: false },
        { help: 'Number of complete presses', input_key: 'count', repeated: false, required: false },
      ],
      description: 'Press and release a key combination.',
      id: 'input.pressKeys',
      target_help: 'Target optional; keyboard input goes to the focused control.',
      target_policy: 'optional_keyboard',
    },
    { arguments: [], description: 'Capture the primary display.', id: 'display.capture', target_help: 'Target forbidden.', target_policy: 'forbidden' },
  ],
}

describe('invokeCatalog', () => {
  // NOTICE: The model never saw `x-auv-commands` (the request had the same
  // 717 input tokens with or without it), so it guessed argument names such
  // as `text` for `query`. The catalog renders the same AUV metadata as text.
  it('renders every command with its input keys and help', () => {
    const catalog = invokeCatalog(schema)

    expect(catalog).toContain('input.pressKeys: Press and release a key combination.')
    expect(catalog).toContain('  inputs.keys (optional, repeated): One key combination. MCP inputs encode this list as a JSON array string')
    expect(catalog).toContain('  target: Target optional; keyboard input goes to the focused control.')
    expect(catalog).toContain('display.capture: Capture the primary display.')
    expect(catalog).not.toContain('Target forbidden.')
  })
})

describe('toolDocsFrom', () => {
  it('defaults to the AUV schema as shipped', () => {
    expect(toolDocsFrom(undefined)).toBe('schema')
    expect(toolDocsFrom('catalog')).toBe('catalog')
    expect(() => toolDocsFrom('prompt')).toThrow(/schema" or "catalog/)
  })
})
