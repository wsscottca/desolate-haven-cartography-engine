// Shared front-end constants. World spans [0, size] in x/y; z is elevation.
export const WORLD = {
  /** World extent in cartographic units (x and y). */
  size: 1000,
  /** Default seed for the placeholder terrain (until the engine lands). */
  seed: 12345,
  /** Vertical exaggeration applied to normalized [-1,1] height for display. */
  heightScale: 120,
};
