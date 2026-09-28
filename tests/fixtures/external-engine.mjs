// Minimal external nesting engine used by engine-registry tests. It implements
// the engine contract (nest(geometry, renderContext, callback, options) ->
// Promise<abort>) without running any placement algorithm.
export async function nest(geometry, renderContext, callback) {
  callback({
    result: [],
    data: { placements: [], index: 0, fitness: 0 },
    status: { better: true, complete: true, placed: 0, total: 0, unplaced: 0 },
  });
  return async () => {};
}
