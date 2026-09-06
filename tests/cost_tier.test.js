// Unit test for OpenRouter Cost tier dropdown and auto-router payload construction
const assert = require('assert');

console.log('=== OpenRouter Cost Tier & Auto-Router Test Suite ===');

// Test 1: OpenRouter model cost tier options generation
{
  function getThinkingOptions(activeModel, modelObj) {
    let efforts = (modelObj && modelObj.reasoning_efforts) || [];
    const isOpenRouterAuto = activeModel && activeModel.startsWith('openrouter/auto');

    if (isOpenRouterAuto && efforts.length === 0) {
      efforts = ['low', 'medium', 'high', 'xhigh', 'max'];
    }

    const label = isOpenRouterAuto ? 'Cost tier' : 'Thinking level';
    const isGemini3 = activeModel && activeModel.startsWith('gemini-3.');
    const options = [];
    if (!efforts.includes('none') && !isGemini3) {
      options.push('off');
    }
    efforts.forEach(lvl => {
      if (!options.includes(lvl)) options.push(lvl);
    });
    return { label, options, visible: efforts.length > 0 };
  }

  // 1. openrouter/auto model
  const autoResult = getThinkingOptions('openrouter/auto', {
    id: 'openrouter/auto',
    provider: 'openrouter',
    reasoning_efforts: ['low', 'medium', 'high', 'xhigh', 'max'],
  });
  assert.strictEqual(autoResult.label, 'Cost tier');
  assert.deepStrictEqual(autoResult.options, ['off', 'low', 'medium', 'high', 'xhigh', 'max']);
  assert.strictEqual(autoResult.visible, true);

  // 2. openrouter/auto-beta model
  const betaResult = getThinkingOptions('openrouter/auto-beta', {
    id: 'openrouter/auto-beta',
    provider: 'openrouter',
    reasoning_efforts: [],
  });
  assert.strictEqual(betaResult.label, 'Cost tier');
  assert.deepStrictEqual(betaResult.options, ['off', 'low', 'medium', 'high', 'xhigh', 'max']);
  assert.strictEqual(betaResult.visible, true);

  // 3. openrouter/fusion model (fusion is deliberation, not auto-router)
  const fusionResult = getThinkingOptions('openrouter/fusion', {
    id: 'openrouter/fusion',
    provider: 'openrouter',
    reasoning_efforts: [],
  });
  assert.strictEqual(fusionResult.label, 'Thinking level');
  assert.strictEqual(fusionResult.visible, false);

  // 4. Regular model with reasoning (e.g. DeepSeek R1)
  const deepseekResult = getThinkingOptions('deepseek/deepseek-r1', {
    id: 'deepseek/deepseek-r1',
    provider: 'openrouter',
    reasoning_efforts: ['low', 'medium', 'high'],
  });
  assert.strictEqual(deepseekResult.label, 'Thinking level');
  assert.deepStrictEqual(deepseekResult.options, ['off', 'low', 'medium', 'high']);
  assert.strictEqual(deepseekResult.visible, true);

  console.log('✓ Test 1 passed: OpenRouter model options and Cost tier label generated correctly');
}

// Test 2: Auto-router plugins payload builder
{
  function buildPayload(model, thinking) {
    const payload = { model };
    const isOpenRouterAuto = model.startsWith('openrouter/auto');
    if (isOpenRouterAuto) {
      if (thinking && thinking !== 'off' && thinking !== 'none') {
        payload.plugins = [{ id: 'auto-router', cost_tier: thinking }];
      }
    } else if (thinking && thinking !== 'off' && thinking !== 'none') {
      payload.reasoning = { enabled: true, effort: thinking };
    }
    return payload;
  }

  // openrouter/auto with medium cost tier
  const mediumPayload = buildPayload('openrouter/auto', 'medium');
  assert.deepStrictEqual(mediumPayload.plugins, [{ id: 'auto-router', cost_tier: 'medium' }]);
  assert.strictEqual(mediumPayload.reasoning, undefined);

  // openrouter/auto-beta with high cost tier
  const betaPayload = buildPayload('openrouter/auto-beta', 'high');
  assert.deepStrictEqual(betaPayload.plugins, [{ id: 'auto-router', cost_tier: 'high' }]);
  assert.strictEqual(betaPayload.reasoning, undefined);

  // openrouter/auto with off cost tier -> NO plugins
  const offPayload = buildPayload('openrouter/auto', 'off');
  assert.strictEqual(offPayload.plugins, undefined);
  assert.strictEqual(offPayload.reasoning, undefined);

  // openrouter/fusion with medium -> uses normal reasoning or no auto-router plugin
  const fusionPayload = buildPayload('openrouter/fusion', 'medium');
  assert.strictEqual(fusionPayload.plugins, undefined);
  assert.deepStrictEqual(fusionPayload.reasoning, { enabled: true, effort: 'medium' });

  // openrouter/auto with max cost tier
  const maxPayload = buildPayload('openrouter/auto', 'max');
  assert.deepStrictEqual(maxPayload.plugins, [{ id: 'auto-router', cost_tier: 'max' }]);

  console.log('✓ Test 2 passed: Auto-router plugins payload generated accurately for all cost tiers');
}

console.log('All Cost Tier tests passed successfully! 🎉');
