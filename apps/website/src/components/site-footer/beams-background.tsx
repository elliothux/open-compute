import { Canvas, useFrame } from "@react-three/fiber";
import {
  forwardRef,
  useEffect,
  useImperativeHandle,
  useMemo,
  useRef,
} from "react";
import * as THREE from "three";

interface BeamsBackgroundProps {
  beamWidth?: number;
  beamHeight?: number;
  beamNumber?: number;
  lightColor?: string;
  beamColor?: string;
  backgroundColor?: string;
  speed?: number;
  noiseIntensity?: number;
  scale?: number;
  rotation?: number;
}

interface BeamMaterialOptions {
  beamColor: string;
  noiseIntensity: number;
  scale: number;
  speed: number;
}

type BeamMesh = THREE.Mesh<THREE.BufferGeometry, THREE.ShaderMaterial>;
type ShaderWithDefines = THREE.ShaderLibShader & {
  defines?: Record<string, string | number | boolean>;
};

const noise = /* glsl */ `
  float random(in vec2 st) {
    return fract(sin(dot(st.xy, vec2(12.9898, 78.233))) * 43758.5453123);
  }

  float noise2d(in vec2 st) {
    vec2 i = floor(st);
    vec2 f = fract(st);
    float a = random(i);
    float b = random(i + vec2(1.0, 0.0));
    float c = random(i + vec2(0.0, 1.0));
    float d = random(i + vec2(1.0, 1.0));
    vec2 u = f * f * (3.0 - 2.0 * f);
    return mix(a, b, u.x) +
      (c - a) * u.y * (1.0 - u.x) +
      (d - b) * u.x * u.y;
  }

  vec4 permute(vec4 x) { return mod(((x * 34.0) + 1.0) * x, 289.0); }
  vec4 taylorInvSqrt(vec4 r) { return 1.79284291400159 - 0.85373472095314 * r; }
  vec3 fade(vec3 t) { return t * t * t * (t * (t * 6.0 - 15.0) + 10.0); }

  float cnoise(vec3 p) {
    vec3 pi0 = floor(p);
    vec3 pi1 = pi0 + vec3(1.0);
    pi0 = mod(pi0, 289.0);
    pi1 = mod(pi1, 289.0);
    vec3 pf0 = fract(p);
    vec3 pf1 = pf0 - vec3(1.0);
    vec4 ix = vec4(pi0.x, pi1.x, pi0.x, pi1.x);
    vec4 iy = vec4(pi0.yy, pi1.yy);
    vec4 iz0 = pi0.zzzz;
    vec4 iz1 = pi1.zzzz;
    vec4 ixy = permute(permute(ix) + iy);
    vec4 ixy0 = permute(ixy + iz0);
    vec4 ixy1 = permute(ixy + iz1);
    vec4 gx0 = ixy0 / 7.0;
    vec4 gy0 = fract(floor(gx0) / 7.0) - 0.5;
    gx0 = fract(gx0);
    vec4 gz0 = vec4(0.5) - abs(gx0) - abs(gy0);
    vec4 sz0 = step(gz0, vec4(0.0));
    gx0 -= sz0 * (step(0.0, gx0) - 0.5);
    gy0 -= sz0 * (step(0.0, gy0) - 0.5);
    vec4 gx1 = ixy1 / 7.0;
    vec4 gy1 = fract(floor(gx1) / 7.0) - 0.5;
    gx1 = fract(gx1);
    vec4 gz1 = vec4(0.5) - abs(gx1) - abs(gy1);
    vec4 sz1 = step(gz1, vec4(0.0));
    gx1 -= sz1 * (step(0.0, gx1) - 0.5);
    gy1 -= sz1 * (step(0.0, gy1) - 0.5);
    vec3 g000 = vec3(gx0.x, gy0.x, gz0.x);
    vec3 g100 = vec3(gx0.y, gy0.y, gz0.y);
    vec3 g010 = vec3(gx0.z, gy0.z, gz0.z);
    vec3 g110 = vec3(gx0.w, gy0.w, gz0.w);
    vec3 g001 = vec3(gx1.x, gy1.x, gz1.x);
    vec3 g101 = vec3(gx1.y, gy1.y, gz1.y);
    vec3 g011 = vec3(gx1.z, gy1.z, gz1.z);
    vec3 g111 = vec3(gx1.w, gy1.w, gz1.w);
    vec4 norm0 = taylorInvSqrt(vec4(dot(g000, g000), dot(g010, g010), dot(g100, g100), dot(g110, g110)));
    g000 *= norm0.x;
    g010 *= norm0.y;
    g100 *= norm0.z;
    g110 *= norm0.w;
    vec4 norm1 = taylorInvSqrt(vec4(dot(g001, g001), dot(g011, g011), dot(g101, g101), dot(g111, g111)));
    g001 *= norm1.x;
    g011 *= norm1.y;
    g101 *= norm1.z;
    g111 *= norm1.w;
    float n000 = dot(g000, pf0);
    float n100 = dot(g100, vec3(pf1.x, pf0.yz));
    float n010 = dot(g010, vec3(pf0.x, pf1.y, pf0.z));
    float n110 = dot(g110, vec3(pf1.xy, pf0.z));
    float n001 = dot(g001, vec3(pf0.xy, pf1.z));
    float n101 = dot(g101, vec3(pf1.x, pf0.y, pf1.z));
    float n011 = dot(g011, vec3(pf0.x, pf1.yz));
    float n111 = dot(g111, pf1);
    vec3 fadeXYZ = fade(pf0);
    vec4 nz = mix(vec4(n000, n100, n010, n110), vec4(n001, n101, n011, n111), fadeXYZ.z);
    vec2 nyz = mix(nz.xy, nz.zw, fadeXYZ.y);
    return 2.2 * mix(nyz.x, nyz.y, fadeXYZ.x);
  }
`;

function createBeamMaterial({
  beamColor,
  noiseIntensity,
  scale,
  speed,
}: BeamMaterialOptions): THREE.ShaderMaterial {
  const physical = THREE.ShaderLib.physical as ShaderWithDefines;
  const uniforms = THREE.UniformsUtils.clone(physical.uniforms) as Record<
    string,
    THREE.IUniform
  >;
  const defaults = new THREE.MeshStandardMaterial({
    color: beamColor,
    metalness: 0.3,
    roughness: 0.3,
  });

  const diffuse = uniforms.diffuse;
  const roughness = uniforms.roughness;
  const metalness = uniforms.metalness;
  if (diffuse) diffuse.value = defaults.color;
  if (roughness) roughness.value = defaults.roughness;
  if (metalness) metalness.value = defaults.metalness;

  uniforms.time = { value: 0 };
  uniforms.uSpeed = { value: speed };
  uniforms.uNoiseIntensity = { value: noiseIntensity };
  uniforms.uScale = { value: scale };

  const header = /* glsl */ `
    varying vec2 vUv;
    uniform float time;
    uniform float uSpeed;
    uniform float uNoiseIntensity;
    uniform float uScale;
    ${noise}
  `;
  const vertexHeader = /* glsl */ `
    float getPos(vec3 pos) {
      vec3 noisePos = vec3(pos.x * 0.0, pos.y - uv.y, pos.z + time * uSpeed * 3.0) * uScale;
      return cnoise(noisePos);
    }
    vec3 getCurrentPos(vec3 pos) {
      vec3 newPos = pos;
      newPos.z += getPos(pos);
      return newPos;
    }
    vec3 getNormal(vec3 pos) {
      vec3 current = getCurrentPos(pos);
      vec3 nextX = getCurrentPos(pos + vec3(0.01, 0.0, 0.0));
      vec3 nextZ = getCurrentPos(pos + vec3(0.0, -0.01, 0.0));
      return normalize(cross(normalize(nextZ - current), normalize(nextX - current)));
    }
  `;

  const vertexShader = `${header}\n${vertexHeader}\n${physical.vertexShader}`
    .replace(
      "#include <begin_vertex>",
      "#include <begin_vertex>\ntransformed.z += getPos(transformed.xyz);",
    )
    .replace(
      "#include <beginnormal_vertex>",
      "#include <beginnormal_vertex>\nobjectNormal = getNormal(position.xyz);",
    );
  const fragmentShader = `${header}\n${physical.fragmentShader}`.replace(
    "#include <dithering_fragment>",
    `#include <dithering_fragment>
      float randomNoise = noise2d(gl_FragCoord.xy);
      gl_FragColor.rgb -= randomNoise / 15.0 * uNoiseIntensity;
      float beamFlow = cnoise(vec3(
        vUv.x * 0.08,
        vUv.y * 0.65,
        time * uSpeed * 0.35
      ));
      float beamLift = 0.012 + pow(
        smoothstep(-0.10, 0.75, beamFlow),
        1.8
      ) * 0.065;
      gl_FragColor.rgb += vec3(beamLift);`,
  );

  return new THREE.ShaderMaterial({
    defines: { ...physical.defines },
    fragmentShader,
    lights: true,
    uniforms,
    vertexShader,
  });
}

function createBeamGeometry(
  count: number,
  width: number,
  height: number,
): THREE.BufferGeometry {
  const heightSegments = 100;
  const vertexCount = count * (heightSegments + 1) * 2;
  const positions = new Float32Array(vertexCount * 3);
  const indices = new Uint32Array(count * heightSegments * 6);
  const uvs = new Float32Array(vertexCount * 2);
  const totalWidth = count * width;
  const xOffsetBase = -totalWidth / 2;
  let vertexOffset = 0;
  let indexOffset = 0;
  let uvOffset = 0;

  for (let beam = 0; beam < count; beam += 1) {
    const xOffset = xOffsetBase + beam * width;
    const uvXOffset = Math.random() * 300;
    const uvYOffset = Math.random() * 300;

    for (let segment = 0; segment <= heightSegments; segment += 1) {
      const y = height * (segment / heightSegments - 0.5);
      positions.set([xOffset, y, 0, xOffset + width, y, 0], vertexOffset * 3);
      const uvY = segment / heightSegments;
      uvs.set(
        [uvXOffset, uvY + uvYOffset, uvXOffset + 1, uvY + uvYOffset],
        uvOffset,
      );

      if (segment < heightSegments) {
        const a = vertexOffset;
        const b = vertexOffset + 1;
        const c = vertexOffset + 2;
        const d = vertexOffset + 3;
        indices.set([a, b, c, c, b, d], indexOffset);
        indexOffset += 6;
      }
      vertexOffset += 2;
      uvOffset += 4;
    }
  }

  const geometry = new THREE.BufferGeometry();
  geometry.setAttribute("position", new THREE.BufferAttribute(positions, 3));
  geometry.setAttribute("uv", new THREE.BufferAttribute(uvs, 2));
  geometry.setIndex(new THREE.BufferAttribute(indices, 1));
  geometry.computeVertexNormals();
  return geometry;
}

const BeamPlanes = forwardRef<
  BeamMesh,
  {
    count: number;
    height: number;
    material: THREE.ShaderMaterial;
    width: number;
  }
>(({ count, height, material, width }, ref) => {
  const meshRef = useRef<BeamMesh>(null);
  useImperativeHandle(ref, () => {
    if (!meshRef.current) throw new Error("Beam mesh is unavailable");
    return meshRef.current;
  });
  const geometry = useMemo(
    () => createBeamGeometry(count, width, height),
    [count, height, width],
  );

  useEffect(() => () => geometry.dispose(), [geometry]);
  useFrame((_, delta) => {
    if (!meshRef.current) return;
    const time = meshRef.current.material.uniforms.time;
    if (time && typeof time.value === "number") time.value += 0.1 * delta;
  });

  return <mesh ref={meshRef} geometry={geometry} material={material} />;
});
BeamPlanes.displayName = "BeamPlanes";

function DirectionalLight({
  color,
  position,
}: {
  color: string;
  position: [number, number, number];
}) {
  const lightRef = useRef<THREE.DirectionalLight>(null);

  useEffect(() => {
    const light = lightRef.current;
    if (!light) return;
    const camera = light.shadow.camera;
    camera.top = 24;
    camera.bottom = -24;
    camera.left = -24;
    camera.right = 24;
    camera.far = 64;
    light.shadow.bias = -0.004;
  }, []);

  return (
    <directionalLight
      ref={lightRef}
      color={color}
      intensity={1}
      position={position}
    />
  );
}

function Scene({
  backgroundColor,
  beamColor,
  beamHeight,
  beamNumber,
  beamWidth,
  lightColor,
  noiseIntensity,
  rotation,
  scale,
  speed,
}: Required<BeamsBackgroundProps>) {
  const meshRef = useRef<BeamMesh>(null);
  const material = useMemo(
    () => createBeamMaterial({ beamColor, noiseIntensity, scale, speed }),
    [beamColor, noiseIntensity, scale, speed],
  );

  useEffect(() => () => material.dispose(), [material]);

  return (
    <>
      <group rotation={[0, 0, THREE.MathUtils.degToRad(rotation)]}>
        <BeamPlanes
          ref={meshRef}
          count={beamNumber}
          height={beamHeight}
          material={material}
          width={beamWidth}
        />
        <DirectionalLight color={lightColor} position={[0, 3, 10]} />
      </group>
      <ambientLight intensity={1} />
      <color attach="background" args={[backgroundColor]} />
    </>
  );
}

// Adapted for the footer from React Bits' Beams background.
// Source: https://reactbits.dev/backgrounds/beams
export function BeamsBackground({
  beamWidth = 2,
  beamHeight = 15,
  beamNumber = 12,
  lightColor = "#ffffff",
  beamColor = "#000000",
  backgroundColor = "#000000",
  speed = 2,
  noiseIntensity = 1.75,
  scale = 0.2,
  rotation = 0,
}: BeamsBackgroundProps) {
  return (
    <div className="footer__beams" aria-hidden="true">
      <Canvas
        camera={{ fov: 30, position: [0, 0, 20] }}
        className="beams-container"
        dpr={[1, 2]}
        frameloop="always"
      >
        <Scene
          backgroundColor={backgroundColor}
          beamColor={beamColor}
          beamHeight={beamHeight}
          beamNumber={beamNumber}
          beamWidth={beamWidth}
          lightColor={lightColor}
          noiseIntensity={noiseIntensity}
          rotation={rotation}
          scale={scale}
          speed={speed}
        />
      </Canvas>
    </div>
  );
}
