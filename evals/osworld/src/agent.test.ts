import { describe, expect, it } from 'vitest'

import { parseAction, toInvocation } from './agent'

describe('parseAction', () => {
  // ROOT CAUSE:
  //
  // If the model appended planned turns as concatenated JSON objects, the
  // reply failed to parse and the step became `wait`.
  //
  // Before the fix, the parser sliced from the first `{` to the last `}`.
  // The fix keeps only the first balanced object as the current decision.
  it('uses the first object when the model concatenates several turns', () => {
    const reply = '{"thought":"a {brace}","action":{"type":"keys","keys":["F2"]}}{"action":{"type":"wait"}}'
    expect(parseAction(reply)).toEqual({ keys: ['F2'], type: 'keys' })
  })

  it('treats a reply without an action as wait', () => {
    expect(parseAction('I cannot see the screen.')).toEqual({ type: 'wait' })
  })
})

describe('toInvocation', () => {
  it('places positionals after -- so typed text cannot become a flag', () => {
    expect(toInvocation({ text: '-rf', type: 'type' })).toEqual(['input.typeText', ['--', '-rf']])
  })
})
