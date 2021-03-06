/*
* Licensed to Elasticsearch B.V. under one or more contributor
* license agreements. See the NOTICE file distributed with
* this work for additional information regarding copyright
* ownership. Elasticsearch B.V. licenses this file to you under
* the Apache License, Version 2.0 (the "License"); you may
* not use this file except in compliance with the License.
* You may obtain a copy of the License at
*
*  http://www.apache.org/licenses/LICENSE-2.0
*
* Unless required by applicable law or agreed to in writing,
* software distributed under the License is distributed on an
* "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
* KIND, either express or implied.  See the License for the
* specific language governing permissions and limitations
* under the License.
*/

use crate::errors::MalwareBrotResult;
use pointcloud::*;
use rand::{thread_rng, Rng};
use std::fmt;

#[derive(Clone)]
pub(crate) struct CoveredData {
    dists: Vec<f32>,
    coverage: Vec<PointIndex>,
    pub(crate) center_index: PointIndex,
}

#[derive(Debug, Clone)]
pub(crate) struct UncoveredData {
    coverage: Vec<PointIndex>,
}

impl UncoveredData {
    pub(crate) fn pick_center<M: Metric>(
        &mut self,
        radius: f32,
        point_cloud: &PointCloud<M>,
    ) -> MalwareBrotResult<CoveredData> {
        let mut rng = thread_rng();
        let new_center: usize = rng.gen_range(0, self.coverage.len());
        let center_index = self.coverage.remove(new_center);
        let dists = point_cloud.distances_to_point_index(center_index, &self.coverage)?;

        let mut close_index = Vec::with_capacity(self.coverage.len());
        let mut close_dist = Vec::with_capacity(self.coverage.len());
        let mut far = Vec::new();
        for (i, d) in self.coverage.iter().zip(&dists) {
            if *d < radius {
                close_index.push(*i);
                close_dist.push(*d);
            } else {
                far.push(*i);
            }
        }
        let close = CoveredData {
            coverage: close_index,
            dists: close_dist,
            center_index,
        };
        self.coverage = far;
        Ok(close)
    }

    pub(crate) fn len(&self) -> usize {
        self.coverage.len()
    }
}

impl fmt::Debug for CoveredData {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "CoveredData {{ center_index: {:?},coverage: {:?} }}",
            self.center_index, self.coverage
        )
    }
}

fn find_split(dist_indexes: &[(f32, usize)], thresh: f32) -> usize {
    let mut smaller = 0;
    let mut larger = dist_indexes.len() - 1;

    while smaller <= larger {
        let m = (smaller + larger) / 2;
        if dist_indexes[m].0 < thresh {
            smaller = m + 1;
        } else if dist_indexes[m].0 > thresh {
            if m == 0 {
                return 0;
            }
            larger = m - 1;
        } else {
            return m;
        }
    }
    smaller
}

impl CoveredData {
    pub(crate) fn new<M: Metric>(point_cloud: &PointCloud<M>) -> MalwareBrotResult<CoveredData> {
        let mut coverage = point_cloud.reference_indexes();
        let center_index = coverage.pop().unwrap();
        let dists = point_cloud.distances_to_point_index(center_index, &coverage)?;
        Ok(CoveredData {
            dists,
            coverage,
            center_index,
        })
    }

    pub(crate) fn split(self, thresh: f32) -> MalwareBrotResult<(CoveredData, UncoveredData)> {
        let mut close_index = Vec::with_capacity(self.coverage.len());
        let mut close_dist = Vec::with_capacity(self.coverage.len());
        let mut far = Vec::new();
        for (i, d) in self.coverage.iter().zip(&self.dists) {
            if *d < thresh {
                close_index.push(*i);
                close_dist.push(*d);
            } else {
                far.push(*i);
            }
        }
        let close = CoveredData {
            coverage: close_index,
            dists: close_dist,
            center_index: self.center_index,
        };
        let new_far = UncoveredData { coverage: far };
        Ok((close, new_far))
    }

    pub(crate) fn to_indexes(self) -> Vec<PointIndex> {
        self.coverage
    }

    pub(crate) fn max_distance(&self) -> f32 {
        self.dists
            .iter()
            .cloned()
            .fold(-1. / 0. /* -inf */, f32::max)
    }

    pub(crate) fn len(&self) -> usize {
        self.coverage.len() + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn splits_correctly_1() {
        let mut data = Vec::with_capacity(20);
        for _i in 0..19 {
            data.push(rand::random::<f32>() + 3.0);
        }
        data.push(0.0);

        let labels: Vec<f32> = data
            .iter()
            .map(|x| if *x > 0.5 { 1.0 } else { 0.0 })
            .collect();

        //data.sort_unstable_by(|a, b| (a).partial_cmp(&b).unwrap_or(Ordering::Equal));

        let point_cloud =
            PointCloud::<L2>::simple_from_ram(Box::from(data), 1, Box::from(labels), 1).unwrap();
        let cache = CoveredData::new(&Arc::new(point_cloud)).unwrap();
        let (close, far) = cache.split(1.0).unwrap();

        assert_eq!(1, close.len());
        assert_eq!(19, far.len());
    }

    #[test]
    fn uncovered_splits_correctly_1() {
        let mut data = Vec::with_capacity(20);
        for _i in 0..19 {
            data.push(rand::random::<f32>() + 3.0);
        }
        data.push(0.0);

        let labels: Vec<f32> = data
            .iter()
            .map(|x| if *x > 0.5 { 1.0 } else { 0.0 })
            .collect();

        //data.sort_unstable_by(|a, b| (a).partial_cmp(&b).unwrap_or(Ordering::Equal));

        let point_cloud =
            PointCloud::<L2>::simple_from_ram(Box::from(data), 1, Box::from(labels), 1).unwrap();
        let mut cache = UncoveredData {
            coverage: (0..19 as PointIndex).collect(),
        };
        let close = cache.pick_center(1.0, &point_cloud).unwrap();

        assert!(!close.coverage.contains(&close.center_index));
        assert!(!cache.coverage.contains(&close.center_index));
        for i in &close.coverage {
            assert!(!cache.coverage.contains(i));
        }
        for i in &cache.coverage {
            assert!(!close.coverage.contains(i));
        }
    }

    #[test]
    fn correct_dists() {
        let mut data = Vec::with_capacity(20);
        for _i in 0..19 {
            data.push(rand::random::<f32>() + 3.0);
        }
        data.push(0.0);

        let labels: Vec<f32> = data
            .iter()
            .map(|x| if *x > 0.5 { 1.0 } else { 0.0 })
            .collect();

        //data.sort_unstable_by(|a, b| (a).partial_cmp(&b).unwrap_or(Ordering::Equal));

        let point_cloud =
            PointCloud::<L2>::simple_from_ram(Box::from(data.clone()), 1, Box::from(labels), 1)
                .unwrap();
        let cache = CoveredData::new(&point_cloud).unwrap();

        let thresh = 0.5;
        let mut true_close: Vec<u64> = Vec::new();
        let mut true_far: Vec<u64> = Vec::new();
        for i in 0..19 {
            if data[i] < thresh {
                true_close.push(i as u64);
            } else {
                true_far.push(i as u64);
            }
            assert_approx_eq!(data[i], cache.dists[i]);
        }
        let (close, _far) = cache.split(thresh).unwrap();

        for (tc, c) in true_close.iter().zip(close.coverage) {
            assert_eq!(*tc, c);
        }
    }
    /*
    #[test]
    fn correct_split_1() {
        for i in 0..100 {
            let mut dist_indexes:Vec<(f32,usize)> = Vec::with_capacity(20);
            for i in 0..2000 {
                dist_indexes.push((rand::random::<f32>(),i));
            }
            dist_indexes.sort_unstable_by(|a, b| (a.0).partial_cmp(&b.0).unwrap_or(Ordering::Equal));
            let thresh = 0.5;
            let split = find_split(&dist_indexes,thresh);
            let (close,far) = dist_indexes.split_at(split);
            for c in close {
                assert!(c.0 < thresh);
            }
            for f in far {
                assert!(f.0 > thresh);
            }
            assert!(close.len() + far.len() == dist_indexes.len());
        }
    }
    */
}
