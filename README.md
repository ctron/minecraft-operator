## Install the operator

You need to install the operator. Once you have installed the operator, you can create a new Minecraft instance by
creating a new custom resource of type `Minecraft`.

### Using OperatorHub

The operator is available on [OperatorHub](https://operatorhub.io/operator/minecraft-operator).

### Using Helm

You can also install the operator using [Helm](https://helm.sh/):

    helm install minecraft-operator ./helm/minecraft-operator

## Create Minecraft instance

Create a new Minecraft instance:

~~~yaml
apiVersion: minecraft.dentrassi.de/v1alpha1
kind: Minecraft
metadata:
  name: test
spec: {}
~~~
