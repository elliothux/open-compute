/** Compile-time coverage for the supported post-beta Vectorize binding. */

declare const VECTOR: Vectorize;

const values: VectorFloatArray | number[] = new Float32Array(32);
const vector: VectorizeVector = {
  id: "document",
  values,
  namespace: "docs",
  metadata: { year: 2026, tags: ["search", "guide"] },
};

async function useVectorize(): Promise<void> {
  const insert: VectorizeAsyncMutation = await VECTOR.insert([vector]);
  insert.mutationId satisfies string;
  (await VECTOR.upsert([vector])).mutationId satisfies string;
  (await VECTOR.deleteByIds([vector.id])).mutationId satisfies string;

  const description: VectorizeIndexInfo = await VECTOR.describe();
  description.vectorCount satisfies number;
  description.dimensions satisfies number;
  description.processedUpToDatetime satisfies number;
  description.processedUpToMutation satisfies number;

  const options: VectorizeQueryOptions = {
    topK: 10,
    namespace: "docs",
    returnValues: true,
    returnMetadata: "all",
    filter: { year: { $gte: 2020, $lt: 2030 } },
  };
  const matches: VectorizeMatches = await VECTOR.query(values, options);
  matches.count satisfies number;
  matches.matches[0]?.score satisfies number | undefined;
  matches.matches[0]?.values satisfies VectorFloatArray | number[] | undefined;
  await VECTOR.queryById(vector.id, options);

  const fetched: VectorizeVector[] = await VECTOR.getByIds([vector.id]);
  fetched[0]?.id satisfies string | undefined;
  fetched[0]?.values satisfies VectorFloatArray | number[] | undefined;
  fetched[0]?.namespace satisfies string | undefined;
  fetched[0]?.metadata satisfies Record<string, VectorizeVectorMetadata> | undefined;
}

void useVectorize;

