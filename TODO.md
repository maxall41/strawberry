- [ ] Cache in compositor.rs and contrast.rs
- [ ] Fallback to CPU for all image processors
- [ ] If compositing > 7 images then just batch into 7 image batches and run each and then composite them together (supports up to 49 images which should be enough for pretty much everything)
- [ ] ImageJ file loading is faster?

## Features:
- [ ] Support dragging and dropping images into viewer
- [ ] Support Z-stacking
- [ ] Support stitching
- [X] Thresholding
- [ ] Watershed segmentation
- [ ] Local maxima detection/segmentation (OPTION: Post processing with signed distance function)
- [ ] Weka segmentation with weka-rs
- [ ] Manual/Autocell counter (Name: Counter, Segmented results get dumped into cell counter tool)
- [ ] Background subtraction
- [ ] Flourescene intensity measurment tool
- [ ] Colocalization analysis tool
- [ ] Saving and loading pipelines
- [ ] Morphological operations (Binary dilation, erosion, opening, closing)

## Big features:
- [ ] Parallel node based editor instead of linear flow
- [ ] Scripting with Rhai: https://github.com/rhaiscript/rhai
