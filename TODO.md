- [ ] Cache in compositor.rs and contrast.rs
- [ ] Fallback to CPU for all image processors
- [ ] If compositing > 7 images then just batch into 7 image batches and run each and then composite them together (supports up to 49 images which should be enough for pretty much everything)
- [ ] Abstract drop menu in tools/manager.rs into GUI component
- [ ] Colors are inverted when loading a single image
- [ ] Update pipeline tool backends to only send Refresh(ThisToolIndex) instead of Refresh(0)

## Features:
- [ ] Support dragging and dropping images into viewer
- [ ] Support Z-stacking
- [ ] Support stitching
- [X] Thresholding
- [ ] Watershed segmentation
- [X] Local maxima detection/segmentation
- [ ] Weka segmentation with weka-rs
- [ ] Manual/Autocell counter (Name: Counter, Segmented results get dumped into cell counter tool)
- [ ] Background subtraction
- [ ] Flourescene intensity measurment tool
- [ ] Colocalization analysis tool
- [ ] Saving and loading pipelines
- [ ] Morphological operations (Binary dilation, erosion, opening, closing)
- [ ] Allow user to type values directly into value control
- [ ] Add mode toggle to "Threshold" that in addition to manual thresholding supports otsu thresholding
- [ ] Settings menu with option to force CPU processing
- [ ] Multiple tool instances
- [ ] Grayscale tool
- [ ] Hold counter state in mode enum so swithcing between auto and manual does not overwrite manual counts

## Big features:
- [ ] Parallel node based editor instead of linear flow
- [ ] Scripting with Rhai: https://github.com/rhaiscript/rhai
- [ ] Plugins
