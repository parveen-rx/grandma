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

//! # The Node
//! This is the workhorse of the library. Each node 
//! 
use crate::errors::{MalwareBrotError, MalwareBrotResult};
use crate::tree_file_format::*;
use crate::query_tools::KnnQueryHeap;
use crate::NodeAddress;
use pointcloud::labels::MetaSummary;
use pointcloud::*;
use smallvec::SmallVec;

/// The node children. This is a separate struct from the `CoverNode` to use the rust compile time type checking and 
/// `Option` data structure to ensure that all nodes with children are valid and cover their nested child.
#[derive(Debug, Clone)]
pub(crate) struct NodeChildren {
    nested_scale: i32,
    addresses: SmallVec<[NodeAddress; 10]>,
}

/// The actual cover node. The fields can be separated into three piles. The first two consist of node `address` for testing and reference
/// when working and the `radius`, `cover_count`, and `singles_summary` for a query various properties of the node.
/// Finally we have the children and singleton pile. The singletons are saved in a `SmallVec` directly attached to the node. This saves a
/// memory redirect for the first 20 singleton children. The children are saved in a separate struct also consisting of a `SmallVec`
/// (though, this is only 10 wide before we allocate on the heap), and the scale index of the nested child.
#[derive(Debug, Clone)]
pub struct CoverNode {
    /// Node address
    address: NodeAddress,
    /// Query caches
    radius: f32,
    cover_count: usize,
    singles_summary: Option<MetaSummary>,
    /// Children
    children: Option<NodeChildren>,
    singles_indexes: SmallVec<[PointIndex; 20]>,
}

impl CoverNode {
    /// Creates a new blank node
    pub fn new(address: NodeAddress) -> CoverNode {
        CoverNode {
            address,
            radius: 0.0,
            cover_count: 0,
            children: None,
            singles_indexes: SmallVec::new(),
            singles_summary: None,
        }
    }

    /// Verifies that this is a leaf by checking there's no nested child
    pub fn is_leaf(&self) -> bool {
        self.children.is_none()
    }

    /// This is currently inconsistent on inserts to children of this node
    pub fn radius(&self) -> f32 {
        self.radius
    }

    /// Add a nested child and converts the node from a leaf to a routing node.
    /// Throws an error if the node is already a routing node with a nested node.
    pub fn insert_nested_child(
        &mut self,
        scale_index: i32,
        coverage: usize,
    ) -> MalwareBrotResult<()> {
        self.cover_count += coverage;
        if let Some(_) = &self.children {
            Err(MalwareBrotError::DoubleNest)
        } else {
            self.children = Some(NodeChildren {
                nested_scale: scale_index,
                addresses: SmallVec::new(),
            });
            Ok(())
        }
    }

    /// Removes all children and returns them to us.
    pub(crate) fn remove_children(&mut self) -> Option<NodeChildren> {
        self.children.take()
    }

    /// The number of singleton points attached to the node
    pub fn singleton_len(&self) -> usize {
        self.singles_indexes.len()
    }

    ///
    pub fn singletons(&self) -> &[PointIndex] {
        &self.singles_indexes
    }

    ///
    pub fn center_index(&self) -> &PointIndex {
        &self.address.1
    }

    ///
    pub fn scale_index(&self) -> &i32 {
        &self.address.0
    }

    /// 
    pub fn children_len(&self) -> usize {
        match &self.children {
            Some(children) => children.addresses.len() + 1,
            None => 0,
        }
    }

    /// If the node is not a leaf this unpacks the child struct to a more publicly consumable format.
    pub fn children(&self) -> Option<(i32, &[NodeAddress])> {
        self.children
            .as_ref()
            .map(|c| (c.nested_scale, &c.addresses[..]))
    }

    /// Performs the `singleton_knn` and `child_knn` with a provided query heap. If you have the distance
    /// from the query point to this you can pass it to save a distance calculation.
    pub fn knn<M: Metric>(
        &self,
        dist_to_center: Option<f32>,
        point: &[f32],
        point_cloud: &PointCloud<M>,
        query_heap: &mut KnnQueryHeap,
    ) -> MalwareBrotResult<()> {
        self.singleton_knn(point, point_cloud, query_heap)?;

        let dist_to_center =
            dist_to_center.unwrap_or(point_cloud.distances_to_point(point, &[self.address.1])?[0]);
        self.child_knn(Some(dist_to_center), point, point_cloud, query_heap)?;

        if self.children.is_none() {
            query_heap.push_outliers(&[self.address.1], &[dist_to_center]);
        }
        Ok(())
    }

    /// Performs a brute force knn against just the singleton children with a provided query heap.
    pub fn singleton_knn<M: Metric>(
        &self,
        point: &[f32],
        point_cloud: &PointCloud<M>,
        query_heap: &mut KnnQueryHeap,
    ) -> MalwareBrotResult<()> {
        let distances = point_cloud.distances_to_point(point, &self.singles_indexes[..])?;
        query_heap.push_outliers(&self.singles_indexes[..], &distances[..]);
        Ok(())
    }

    /// Performs a brute force knn against the children of the node with a provided query heap. Does nothing if this is a leaf node.
    /// If you have the distance from the query point to this you can pass it to save a distance calculation.
    pub fn child_knn<M: Metric>(
        &self,
        dist_to_center: Option<f32>,
        point: &[f32],
        point_cloud: &PointCloud<M>,
        query_heap: &mut KnnQueryHeap,
    ) -> MalwareBrotResult<()> {
        let dist_to_center =
            dist_to_center.unwrap_or(point_cloud.distances_to_point(point, &[self.address.1])?[0]);

        if let Some(children) = &self.children {
            query_heap.push_nodes(
                &[(children.nested_scale, self.address.1)],
                &[dist_to_center],
                None,
            );
            let children_indexes: Vec<PointIndex> =
                children.addresses.iter().map(|(_si, pi)| *pi).collect();
            let distances = point_cloud.distances_to_point(point, &children_indexes[..])?;
            query_heap.push_nodes(&children.addresses[..], &distances, Some(self.address));
        }
        Ok(())
    }

    /// Inserts a routing child into the node. Make sure the child node is also in the tree or you get a dangling reference
    pub(crate) fn insert_child(&mut self, address: NodeAddress, coverage: usize) -> MalwareBrotResult<()> {
        self.cover_count += coverage;
        if let Some(children) = &mut self.children {
            children.addresses.push(address);
            Ok(())
        } else {
            Err(MalwareBrotError::InsertBeforeNest)
        }
    }

    /// Inserts a `vec` of singleton children into the node.
    pub(crate) fn insert_singletons(&mut self, addresses: Vec<PointIndex>) {
        self.cover_count += addresses.len();
        self.singles_indexes.extend(addresses);
    }
    /// Inserts a single singleton child into the node.
    pub(crate) fn insert_singleton(&mut self, address: PointIndex) {
        self.cover_count += 1;
        self.singles_indexes.push(address);
    }
    /// Updates the radius
    pub(crate) fn set_radius(&mut self, radius: f32) {
        self.radius = radius;
    }

    /// Updates the metasummary of the singletons this covers. Call this after inserting or removing a singleton.
    pub(crate) fn update_metasummary<M: Metric>(
        &mut self,
        point_cloud: &PointCloud<M>,
    ) -> MalwareBrotResult<()> {
        self.singles_summary = Some(point_cloud.get_metasummary(&self.singles_indexes[..])?);
        Ok(())
    }

    pub(crate) fn load(scale_index: i32, node_proto: &NodeProto) -> CoverNode {
        let singles_indexes = node_proto
            .outlier_point_indexes
            .iter()
            .map(|i| *i as PointIndex)
            .collect();
        let singles_summary = Some(MetaSummary::new());
        let radius = node_proto.get_radius();
        let address = (scale_index, node_proto.get_center_index());
        let cover_count = node_proto.get_cover_count() as usize;
        let children;
        if node_proto.get_is_leaf() {
            children = None;
        } else {
            let nested_scale = node_proto.get_nested_scale_index() as i32;
            let addresses = node_proto
                .get_children_scale_indexes()
                .iter()
                .zip(node_proto.get_children_point_indexes())
                .map(|(si, pi)| (*si as i32, *pi as PointIndex))
                .collect();
            children = Some(NodeChildren {
                nested_scale,
                addresses,
            });
        }
        CoverNode {
            address,
            radius,
            cover_count,
            children,
            singles_indexes,
            singles_summary,
        }
    }

    pub(crate) fn save(&self) -> NodeProto {
        let mut proto = NodeProto::new();
        proto.set_cover_count(self.cover_count as u64);
        proto.set_center_index(self.address.1 as u64);
        proto.set_radius(self.radius);
        proto.set_outlier_point_indexes(self.singles_indexes.iter().map(|pi| *pi as u64).collect());

        match &self.children {
            Some(children) => {
                proto.set_is_leaf(false);
                proto.set_nested_scale_index(children.nested_scale);
                proto.set_children_scale_indexes(
                    children.addresses.iter().map(|(si, _pi)| *si).collect(),
                );
                proto.set_children_point_indexes(
                    children
                        .addresses
                        .iter()
                        .map(|(_si, pi)| *pi as u64)
                        .collect(),
                );
            }
            None => proto.set_is_leaf(true),
        }
        proto
    }

    /// Brute force verifies that the children are separated by at least the scale provided. 
    /// The scale provided should be b^(s-1) where s is this node's scale index.
    pub fn check_seperation<M: Metric>(
        &self,
        scale: f32,
        point_cloud: &PointCloud<M>,
    ) -> MalwareBrotResult<bool> {
        let mut nodes = self.singles_indexes.clone();
        nodes.push(self.address.1);
        if let Some(children) = &self.children {
            nodes.extend(children.addresses.iter().map(|(_si, pi)| *pi));
        }
        let adj = point_cloud.adj(&nodes)?;
        Ok(scale > adj.min())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::tree::tests::build_mnist_tree;
    use crate::query_tools::tests::clone_unvisited_nodes;
    use crate::query_tools::query_items::QueryAddress;

    fn create_test_node() -> CoverNode {
        let children = Some(NodeChildren {
            nested_scale: 0,
            addresses: smallvec![(-4, 1), (-4, 2), (-4, 3)],
        });

        CoverNode {
            address: (0, 0),
            radius: 1.0,
            cover_count: 8,
            children,
            singles_indexes: smallvec![4, 5, 6],
            singles_summary: None,
        }
    }

    fn create_test_leaf_node() -> CoverNode {
        CoverNode {
            address: (0, 0),
            radius: 1.0,
            cover_count: 8,
            children: None,
            singles_indexes: smallvec![1, 2, 3, 4, 5, 6],
            singles_summary: None,
        }
    }

    #[test]
    fn knn_node_children_mixed() {
        // Tests the mixed uppacking
        let data = vec![0.0, 0.49, 0.48, 0.5, 0.1, 0.2, 0.3];
        let labels = vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0];

        let point_cloud =
            PointCloud::<L2>::simple_from_ram(Box::from(data), 1, Box::from(labels), 1).unwrap();

        let test_node = create_test_node();
        let mut heap = KnnQueryHeap::new(5,2.0);
        let point = [0.494];
        test_node
            .knn(None, &point, &point_cloud, &mut heap)
            .unwrap();
        println!("{:?}", heap);
        println!("There shoud be 4 node addresses on the heap here");
        assert!(heap.node_len() == 4);
        println!("There shoud be only 3 singleton indexes on the heap");
        assert!(heap.len() == 5);
        let results = heap.unpack();
        println!("There should be 5 results, {:?}", results);
        assert!(results.len() == 5);
        println!("The first result should be 1 but is {:?}", results[0].1);
        assert!(results[0].1 == 1);
        println!("The first result should be 3 but is {:?}", results[1].1);
        assert!(results[1].1 == 3);
    }

    #[test]
    fn knn_node_children_only() {
        let data = vec![0.0, 0.49, 0.48, 0.5, 0.1, 0.2, 0.3];
        let labels = vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0];

        let point_cloud =
            PointCloud::<L2>::simple_from_ram(Box::from(data), 1, Box::from(labels), 1).unwrap();

        let test_node = create_test_node();
        let mut heap = KnnQueryHeap::new(5,2.0);
        let point = [0.494];
        test_node
            .knn(None, &point, &point_cloud, &mut heap)
            .unwrap();
        println!("{:?}", heap);
        println!("There shoud be 4 node addresses on the heap here");
        assert!(heap.node_len() == 4);
        println!("There shoud be only 3 singleton indexes on the heap");
        assert!(heap.len() == 5);
        let results = heap.unpack();
        println!("There should be 5 results, {:?}", results);
        assert!(results.len() == 5);
        println!("The first result should be 1 but is {:?}", results[0].1);
        assert!((results[0].1) == 1);
        println!("The first result should be 3 but is {:?}", results[1].1);
        assert!((results[1].1) == 3);
    }

    #[test]
    fn knn_node_leaf() {
        let data = vec![0.0, 0.49, 0.48, 0.5, 0.1, 0.2, 0.3];
        let labels = vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0];

        let point_cloud =
            PointCloud::<L2>::simple_from_ram(Box::from(data), 1, Box::from(labels), 1).unwrap();

        let test_node = create_test_leaf_node();
        let mut heap = KnnQueryHeap::new(5,2.0);
        let point = [0.494];
        test_node
            .knn(None, &point, &point_cloud, &mut heap)
            .unwrap();
        println!("{:?}", heap);
        println!("There shoudn't be any node addresses on the heap here");
        assert!(heap.node_len() == 0);
        println!("There shoud be only 2 singleton indexes on the heap");
        assert!(heap.len() == 5);
        let results = heap.unpack();
        println!("There should be 5 results");
        assert!(results.len() == 5);
        println!("The first result should be 1 but is {:?}", results[0].1);
        assert!(results[0].1 == 1);
        println!("The first result should be 3 but is {:?}", results[1].1);
        assert!(results[1].1 == 3);
    }

    fn brute_test_knn_node<M: Metric>(node: &CoverNode, point_cloud: &PointCloud<M>) {
        let zeros: Vec<f32> = vec![0.0; 784];

        let mut all_children = Vec::from(node.singletons());
        if let Some(children) = &node.children {
            all_children.extend(children.addresses.iter().map(|(_si,pi)| *pi));
        }
        all_children.push(node.address.1);

        let brute_knn = point_cloud
            .distances_to_point(&zeros, &all_children)
            .unwrap();
        let mut brute_knn: Vec<(f32, PointIndex)> = brute_knn
            .iter()
            .zip(all_children)
            .map(|(d, i)| (*d, i))
            .collect();
        brute_knn.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());

        let children = match &node.children() {
            Some((si, ca)) => {
                let mut c: Vec<NodeAddress> = ca.iter().cloned().collect();
                c.push((*si, *node.center_index()));
                c
            }
            None => vec![],
        };
        let children_indexes: Vec<PointIndex> = children.iter().map(|(_si, pi)| *pi).collect();

        let children_dist = point_cloud
            .distances_to_point(&zeros, &children_indexes)
            .unwrap();
        let mut children_range_calc: Vec<QueryAddress> = children_dist
            .iter()
            .zip(children)
            .map(|(d, (si,pi))| QueryAddress {min_dist:(*d - (1.3f32).powi(si)).max(0.0), dist_to_center:*d, address:(si,pi)})
            .collect();
        children_range_calc.sort();

        let mut heap = KnnQueryHeap::new(10000,1.3);
        node.knn(None, &zeros, &point_cloud, &mut heap).unwrap();

        let heap_range: Vec<NodeAddress> = clone_unvisited_nodes(&heap).iter().map(|(_d,a)| *a).collect();
        let heap_knn: Vec<PointIndex> = heap.unpack().iter().map(|(_d,pi)| *pi).collect();

        let children_range_calc: Vec<NodeAddress> = children_range_calc.iter().map(|a| a.address).collect();
        let brute_knn: Vec<PointIndex> = brute_knn.iter().map(|(_d,pi)| *pi).collect();

        let mut correct = true;
        if correct {
            correct = heap_knn == brute_knn;
        }
        if correct {
            correct = heap_range == children_range_calc;
        }
        if !correct {
            println!("Node: {:?}", node);
            println!("=============");
            println!("Heap Range Calc: {:?}", heap_range);
            println!("Brute Range Calc: {:?}", children_range_calc);
            println!("Heap Knn: {:?}", heap_knn);
            println!("Brute Knn: {:?}", brute_knn);

            assert!(false);
        }
    }

    #[test]
    fn mnist_knn_node_on_level() {
        let tree = build_mnist_tree();
        let reader = tree.reader();
        println!("Testing Root");
        reader
            .get_node_and(reader.root_address(), |n| {
                brute_test_knn_node(n, reader.point_cloud())
            })
            .unwrap();

        let layer = reader.layer(reader.root_address().0 - 3);
        println!(
            "Testing 3 layers below root, with {} nodes",
            layer.node_count()
        );
        layer.for_each_node(|_, n| brute_test_knn_node(n, reader.point_cloud()));
    }
}
